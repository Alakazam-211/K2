//! What a structured call costs on a live keyed doc that isn't a table (a deck, a board): it
//! should cost what it touches, not the whole doc.
//!
//! The test pack, `deck-cost`: a MovableList `shapes` and a Map `fields` keyed
//! `<shape>:<field>`; the file is one line per shape (its key, then `\t<field>=<value>` per
//! field). It keeps a carrier: `encode_from` keeps a shape's line when its fields are what
//! the carrier says. Its verbs: `shape get <shape>` (a read of one shape's fields), `shape set
//! <shape> <field> <value>` (one field), `shape add <shape> <after>` (one element). It counts
//! its renders (`encode` and `encode_from` calls).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential;
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::test_support::{carrier_judge, CarrierVerdict};
use k2_mpc_base::session::{
    AuditEvent, ChangeEvent, Config, ConnId, Declined, Host, Pack, PackContent, PackError,
    Participant, ParticipantKind, Rights, Session, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError, ToolOutput};

// ---------- the test pack ----------

const SHAPES: &str = "shapes";
const FIELDS: &str = "fields";

const DECK: Schema = Schema {
    name: "deck-cost",
    roots: &[(SHAPES, Kind::MovableList), (FIELDS, Kind::Map)],
};

/// Fields per shape in the generated files.
const PER_SHAPE: usize = 7;

struct Deck {
    /// Every `encode` and `encode_from` call (a test-only count, never a timing).
    renders: AtomicU64,
}

impl Deck {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            renders: AtomicU64::new(0),
        })
    }
    fn renders(&self) -> u64 {
        self.renders.load(Ordering::SeqCst)
    }
}

fn bad(message: String) -> Declined {
    Declined {
        code: "malformed",
        message,
    }
}

/// A file's shapes: each key, its fields, and its line (newline included).
type Lines<'a> = Vec<(Key, BTreeMap<String, String>, &'a str)>;

/// Each shape's key and fields, in order.
type Shapes = Vec<(Key, BTreeMap<String, String>)>;

/// A file's shapes ([`Lines`]).
fn parse(b: &[u8]) -> Result<Lines<'_>, Declined> {
    let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
    let mut out = Vec::new();
    for raw in s.split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let mut parts = line.split('\t');
        let key = Key::new(parts.next().unwrap_or("")).map_err(|e| bad(e.to_string()))?;
        let mut fields = BTreeMap::new();
        for p in parts {
            let (f, v) = p
                .split_once('=')
                .ok_or_else(|| bad(format!("no = in {p:?}")))?;
            fields.insert(f.to_owned(), v.to_owned());
        }
        out.push((key, fields, raw));
    }
    Ok(out)
}

/// Each shape's key and fields, in order, from the doc's content.
fn shapes_of(c: &[(String, Neutral)]) -> Result<Shapes, PackError> {
    let mut shapes: &[(Key, Value)] = &[];
    let mut fields = &BTreeMap::new();
    for (r, n) in c {
        match n {
            Neutral::List(l) if r == SHAPES => shapes = l,
            Neutral::Map(m) if r == FIELDS => fields = m,
            _ => return Err(PackError(format!("unexpected root {r}"))),
        }
    }
    Ok(shapes
        .iter()
        .map(|(k, _)| {
            let prefix = format!("{}:", k.as_str());
            let mine = fields
                .range(prefix.clone()..)
                .map_while(|(fk, v)| {
                    let f = fk.strip_prefix(&prefix)?;
                    Some((f.to_owned(), v.as_str().unwrap_or("").to_owned()))
                })
                .collect();
            (k.clone(), mine)
        })
        .collect())
}

fn canonical(k: &Key, fields: &BTreeMap<String, String>) -> String {
    let mut out = k.as_str().to_owned();
    for (f, v) in fields {
        out.push('\t');
        out.push_str(f);
        out.push('=');
        out.push_str(v);
    }
    out.push('\n');
    out
}

impl Pack for Deck {
    fn schema(&self) -> Schema {
        DECK
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let mut list = Vec::new();
        let mut fields = BTreeMap::new();
        for (k, f, _) in parse(b)? {
            for (name, v) in &f {
                fields.insert(format!("{}:{name}", k.as_str()), Value::str(v));
            }
            list.push((k, Value::Null));
        }
        Ok(PackContent {
            roots: vec![
                (SHAPES.into(), Neutral::List(list)),
                (FIELDS.into(), Neutral::Map(fields)),
            ],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        self.renders.fetch_add(1, Ordering::SeqCst);
        Ok(shapes_of(c)?
            .iter()
            .map(|(k, f)| canonical(k, f))
            .collect::<String>()
            .into_bytes())
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
    fn wants_carrier(&self) -> bool {
        true
    }
    fn encode_from(
        &self,
        carrier: &[u8],
        c: &[(String, Neutral)],
        _: &[u8],
    ) -> Result<Vec<u8>, PackError> {
        self.renders.fetch_add(1, Ordering::SeqCst);
        let old = parse(carrier).map_err(|d| PackError(d.message))?;
        let by_key: HashMap<&Key, (&BTreeMap<String, String>, &str)> =
            old.iter().map(|(k, f, raw)| (k, (f, *raw))).collect();
        let mut out = String::with_capacity(carrier.len());
        for (k, fields) in shapes_of(c)? {
            match by_key.get(&k) {
                Some((f, raw)) if **f == fields => out.push_str(raw),
                _ => out.push_str(&canonical(&k, &fields)),
            }
        }
        Ok(out.into_bytes())
    }
}

fn usage(m: &str) -> ToolError {
    ToolError::usage(m.to_owned())
}

/// `shape get <shape>`: one shape's fields, `<field>=<value>` per line (exit 4: no such shape).
fn shape_get(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape] = args else {
        return Err(usage("shape get <shape>"));
    };
    let prefix = format!("{shape}:");
    Ok(Box::new(move |ctx| {
        let mut out = String::new();
        let empty = BTreeMap::new();
        for (k, v) in ctx
            .view()
            .map(FIELDS)
            .unwrap_or(&empty)
            .range(prefix.clone()..)
        {
            let Some(f) = k.strip_prefix(&prefix) else {
                break;
            };
            out.push_str(f);
            out.push('=');
            out.push_str(v.as_str().unwrap_or(""));
            out.push('\n');
        }
        let exit = if out.is_empty() { 4 } else { 0 };
        Ok(StructOutput::new(Vec::new(), out.into_bytes(), exit))
    }))
}

/// `shape set <shape> <field> <value>`: one field (exit 4: no such shape).
fn shape_set(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape, field, value] = args else {
        return Err(usage("shape set <shape> <field> <value>"));
    };
    let (key, value) = (format!("{shape}:{field}"), value.clone());
    let probe = format!("{shape}:");
    Ok(Box::new(move |ctx| {
        let empty = BTreeMap::new();
        let known = ctx
            .view()
            .map(FIELDS)
            .unwrap_or(&empty)
            .range(probe.clone()..)
            .next()
            .is_some_and(|(k, _)| k.starts_with(&probe));
        if !known {
            return Ok(StructOutput::new(
                Vec::new(),
                b"no such shape\n".to_vec(),
                4,
            ));
        }
        Ok(StructOutput::new(
            vec![Edit::Map {
                root: FIELDS.into(),
                ops: vec![MapOp::Set {
                    key: key.clone(),
                    value: Value::str(&value),
                }],
            }],
            Vec::new(),
            0,
        ))
    }))
}

/// `shape add <shape> <after>`: one element, after `<after>` (`-`: first).
fn shape_add(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape, after] = args else {
        return Err(usage("shape add <shape> <after>"));
    };
    let key = Key::new(shape).map_err(|e| usage(&e.to_string()))?;
    let at = match after.as_str() {
        "-" => Anchor::Start,
        a => Anchor::After(Key::new(a).map_err(|e| usage(&e.to_string()))?),
    };
    Ok(Box::new(move |_| {
        Ok(StructOutput::new(
            vec![Edit::List {
                root: SHAPES.into(),
                ops: vec![ListOp::Insert {
                    key: key.clone(),
                    at: at.clone(),
                    value: Value::Null,
                }],
            }],
            Vec::new(),
            0,
        ))
    }))
}

/// `shape move <shape> <after>`: move one element after `<after>` (`-`: first).
fn shape_move(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape, after] = args else {
        return Err(usage("shape move <shape> <after>"));
    };
    let key = Key::new(shape).map_err(|e| usage(&e.to_string()))?;
    let to = match after.as_str() {
        "-" => Anchor::Start,
        a => Anchor::After(Key::new(a).map_err(|e| usage(&e.to_string()))?),
    };
    Ok(Box::new(move |_| {
        Ok(StructOutput::new(
            vec![Edit::List {
                root: SHAPES.into(),
                ops: vec![ListOp::Move {
                    key: key.clone(),
                    to: to.clone(),
                }],
            }],
            Vec::new(),
            0,
        ))
    }))
}

/// `shape drop <shape>`: delete one element.
fn shape_drop(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape] = args else {
        return Err(usage("shape drop <shape>"));
    };
    let key = Key::new(shape).map_err(|e| usage(&e.to_string()))?;
    Ok(Box::new(move |_| {
        Ok(StructOutput::new(
            vec![Edit::List {
                root: SHAPES.into(),
                ops: vec![ListOp::Delete { key: key.clone() }],
            }],
            Vec::new(),
            0,
        ))
    }))
}

/// `shape at <#N> <field> <value>`: one field of the shape at position N (1-based) in the
/// version the call names (`--base`, R-1), exit 4 past the end.
fn shape_at(args: &[String]) -> Result<StructEdit, ToolError> {
    let [at, field, value] = args else {
        return Err(usage("shape at <#N> <field> <value>"));
    };
    let n: usize = at
        .strip_prefix('#')
        .and_then(|n| n.parse().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| usage("shape at: #N"))?;
    let (field, value) = (field.clone(), value.clone());
    Ok(Box::new(move |ctx| {
        let shapes = ctx.positions_view().list(SHAPES).unwrap_or(&[]);
        let Some((k, _)) = shapes.get(n - 1) else {
            return Ok(StructOutput::new(
                Vec::new(),
                b"no such shape\n".to_vec(),
                4,
            ));
        };
        Ok(StructOutput::new(
            vec![Edit::Map {
                root: FIELDS.into(),
                ops: vec![MapOp::Set {
                    key: format!("{}:{field}", k.as_str()),
                    value: Value::str(&value),
                }],
            }],
            Vec::new(),
            0,
        ))
    }))
}

/// [`shape_at`]'s positional address: its first argument.
fn shape_at_position(args: &[String]) -> Option<String> {
    args.first().filter(|a| a.starts_with('#')).cloned()
}

const VERBS: &[PackVerb] = &[
    PackVerb::structured_read(
        "shape get",
        "shape get <shape>",
        &[PersonAction::ReadAll],
        &[0, 4],
        shape_get,
    ),
    PackVerb::structured_edit(
        "shape set",
        "shape set <shape> <field> <value>",
        &[PersonAction::ReadAll],
        &[0, 4],
        shape_set,
    ),
    PackVerb::structured_edit(
        "shape add",
        "shape add <shape> <after>",
        &[PersonAction::ReadAll],
        &[0],
        shape_add,
    ),
    PackVerb::structured_edit(
        "shape move",
        "shape move <shape> <after>",
        &[PersonAction::ReadAll],
        &[0],
        shape_move,
    ),
    PackVerb::structured_edit(
        "shape drop",
        "shape drop <shape>",
        &[PersonAction::ReadAll],
        &[0],
        shape_drop,
    ),
    PackVerb::structured_edit(
        "shape at",
        "shape at <#N> <field> <value>",
        &[PersonAction::ReadAll],
        &[0, 4],
        shape_at,
    )
    .positional(shape_at_position),
];

// ---------- harness ----------

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

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    mgr: SessionManager,
    pack: Arc<Deck>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

const NAME: &str = "talk.deck";

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-keyed-cost-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        // 2026-10-27 14:30:00 UTC
        now: AtomicU64::new(1_793_111_400_000),
    });
    let store = Arc::new(DocStore::open(p.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), config());
    Fx {
        dir: p,
        ws,
        host,
        store,
        mgr,
        pack: Deck::new(),
    }
}

fn config() -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c.keyed_max_entries = 1_000_000;
    c
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: NAME.into(),
    }
}

impl Fx {
    fn path(&self) -> PathBuf {
        self.ws.join(NAME)
    }
    fn open(&self) -> Arc<Session> {
        self.mgr
            .open_file(self.pack.clone(), &key(), &self.path(), &self.ws)
            .unwrap()
    }
    fn agent(&self, s: &Session) -> ConnId {
        s.join(
            Participant::new("bot", "bot", "#123456", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    /// A restart: a new manager on the same store, the old one dropped.
    fn restart(&mut self) {
        self.mgr = SessionManager::new(self.store.clone(), self.host.clone(), config());
    }
}

/// A deck file of `shapes` shapes (`s0`, `s1`, ...), each with [`PER_SHAPE`] fields.
fn deck(shapes: usize) -> String {
    let mut out = String::new();
    for i in 0..shapes {
        out.push_str(&format!("s{i}"));
        for f in 0..PER_SHAPE {
            out.push_str(&format!("\tf{f}=value {f} of shape {i}"));
        }
        out.push('\n');
    }
    out
}

fn run(s: &Session, c: ConnId, argv: &[&str]) -> ToolOutput {
    let argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
    let o = s.run_tool(c, FIELDS, &argv, b"").expect("the verb runs");
    assert_eq!(
        o.exit,
        0,
        "{argv:?}: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    o
}

// ---------- the rows: test-only counts, not timing ----------

/// The most entries a one-entry call may visit (read or copy in bulk), whatever the doc's size.
const MAX_VISITS: u64 = 16;

/// The doc sizes the visit rows run at (elements and fields).
const SIZES: [usize; 2] = [1_000, 50_000];

/// Switches the live view's checks off on this thread while it lives (they read the whole doc
/// on purpose), and back to what they were after.
struct NoChecks(bool);

impl NoChecks {
    fn new() -> Self {
        Self(differential::set_view_checks(false))
    }
}

impl Drop for NoChecks {
    fn drop(&mut self) {
        differential::set_view_checks(self.0);
    }
}

type Events = std::sync::mpsc::Receiver<ChangeEvent>;

/// A live doc of about `entries` elements and fields, opened, with an agent that has read it
/// and written twice (so whatever the kit builds on first need is built) and a change
/// subscriber. The visit count is taken (reset) last.
fn warmed(entries: usize) -> (Fx, Arc<Session>, ConnId, Events) {
    let f = fx("visits");
    fs::write(f.path(), deck(entries / (PER_SHAPE + 1))).unwrap();
    let s = f.open();
    let a = f.agent(&s);
    let events = s.subscribe(a).unwrap();
    for i in 0..2 {
        let v = run(&s, a, &["shape", "get", "s1"]).version.to_string();
        let val = format!("warm {i}");
        run(&s, a, &["shape", "set", "s1", "f0", &val, "--base", &v]);
    }
    while events.try_recv().is_ok() {}
    differential::take_entry_visits();
    (f, s, a, events)
}

/// The visits of an agent's one-shape read, then of its one-field verb (with `--base` from
/// that read), on a warmed doc of about `entries` elements and fields.
fn one_entry_visits(entries: usize) -> (u64, u64) {
    let _off = NoChecks::new();
    let (_f, s, a, events) = warmed(entries);
    let shape = format!("s{}", entries / (PER_SHAPE + 1) / 2);
    let v = run(&s, a, &["shape", "get", &shape]).version.to_string();
    let read = differential::take_entry_visits();
    run(
        &s,
        a,
        &["shape", "set", &shape, "f1", "moved", "--base", &v],
    );
    let verb = differential::take_entry_visits();
    assert!(events.try_recv().is_ok(), "the verb's change event");
    (read, verb)
}

/// **read visits**: an agent's one-shape read (`shape get`) visits at most [`MAX_VISITS`]
/// entries at 1,000 and at 50,000 elements and fields.
fn row_read_visits() -> Result<(), String> {
    for n in SIZES {
        let (read, _) = one_entry_visits(n);
        if read > MAX_VISITS {
            return Err(format!(
                "read visits: a one-shape read visited {read} entries at {n} elements and fields (at most {MAX_VISITS})"
            ));
        }
    }
    Ok(())
}

/// **verb visits**: an agent's one-field verb (`shape set --base`), its apply and its change
/// event, visit at most [`MAX_VISITS`] entries at 1,000 and at 50,000 elements and fields.
fn row_verb_visits() -> Result<(), String> {
    for n in SIZES {
        let (_, verb) = one_entry_visits(n);
        if verb > MAX_VISITS {
            return Err(format!(
                "verb visits: a one-field verb visited {verb} entries at {n} elements and fields (at most {MAX_VISITS})"
            ));
        }
    }
    Ok(())
}

/// The kit's visits of an agent's one-element list changes on a warmed doc of about `entries`
/// elements and fields: an insert (`shape add`), a move (`shape move`) and a delete (`shape
/// drop`), each with `--base` from a read just before. The engine's own work (its list plan's
/// read, its order) is counted apart (`take_engine_visits`) and not here.
fn list_visits(entries: usize) -> [(&'static str, u64); 3] {
    let _off = NoChecks::new();
    let (_f, s, a, events) = warmed(entries);
    let n = entries / (PER_SHAPE + 1);
    let calls: [Vec<String>; 3] = [
        vec!["add".into(), "x1".into(), format!("s{}", n / 2)],
        vec!["move".into(), format!("s{}", n / 3), format!("s{}", n / 2)],
        vec!["drop".into(), format!("s{}", n / 4)],
    ];
    let mut out = [("insert", 0), ("move", 0), ("delete", 0)];
    for (i, call) in calls.iter().enumerate() {
        let v = run(&s, a, &["shape", "get", "s1"]).version.to_string();
        differential::take_entry_visits();
        let mut argv = vec!["shape"];
        argv.extend(call.iter().map(String::as_str));
        argv.extend(["--base", v.as_str()]);
        run(&s, a, &argv);
        out[i].1 = differential::take_entry_visits();
    }
    assert!(events.try_recv().is_ok(), "the verbs' change events");
    out
}

/// **list visits**: an agent's one-element list insert, move and delete each visit at most
/// [`MAX_VISITS`] of the kit's entries at 1,000 and at 50,000 elements and fields.
fn row_list_visits() -> Result<(), String> {
    for n in SIZES {
        for (op, visits) in list_visits(n) {
            if visits > MAX_VISITS {
                return Err(format!(
                    "list visits: a one-element {op} visited {visits} entries at {n} elements and fields (at most {MAX_VISITS})"
                ));
            }
        }
    }
    Ok(())
}

/// **open renders**: opening a file nobody touched renders nothing (the pack's `encode` and
/// `encode_from` are never called): the first open, which imports it, and a reopen after a
/// write-back and a restart. The file is what the sync point says, so there is nothing to
/// compare.
fn row_open_renders() -> Result<(), String> {
    let _off = NoChecks::new();
    let mut f = fx("open");
    fs::write(f.path(), deck(125)).unwrap();
    let r0 = f.pack.renders();
    let s = f.open();
    let n = f.pack.renders() - r0;
    if n != 0 {
        return Err(format!(
            "open renders: the first open rendered the doc {n} times"
        ));
    }
    let a = f.agent(&s);
    let v = run(&s, a, &["shape", "get", "s3"]).version.to_string();
    run(
        &s,
        a,
        &["shape", "set", "s3", "f2", "written", "--base", &v],
    );
    let r0 = f.pack.renders();
    assert!(s.flush(a).expect("the write-back").written);
    assert_eq!(f.pack.renders() - r0, 1, "a write-back renders once");
    assert!(fs::read_to_string(f.path()).unwrap().contains("f2=written"));
    drop(s);
    f.mgr.close(&key(), "test").unwrap();
    f.restart();
    let r0 = f.pack.renders();
    let s = f.open();
    let n = f.pack.renders() - r0;
    if n != 0 {
        return Err(format!(
            "open renders: a reopen of the file the kit wrote rendered the doc {n} times"
        ));
    }
    let a = f.agent(&s);
    let o = run(&s, a, &["shape", "get", "s3"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("f2=written"));
    Ok(())
}

/// A read base of a keyed doc is its version alone, rebuilt from the live view and the journal
/// when a verb names positions at it: after a list insert, a field set and a list delete since
/// the read, `#3 --base <read>` is the shape third at the read (the live doc's third is another),
/// and the test build checks the rebuilt base against the snapshot taken with the read.
#[test]
fn a_keyed_read_base_is_rebuilt_from_the_journal() {
    assert!(differential::view_checks(), "the checks are on by default");
    let f = fx("base");
    fs::write(f.path(), deck(6)).unwrap();
    let s = f.open();
    let a = f.agent(&s);
    let v1 = run(&s, a, &["shape", "get", "s0"]).version.to_string();
    let v = run(&s, a, &["shape", "add", "n1", "s0", "--base", &v1])
        .version
        .to_string();
    let v = run(&s, a, &["shape", "set", "s1", "f0", "later", "--base", &v])
        .version
        .to_string();
    run(&s, a, &["shape", "drop", "s2", "--base", &v]);
    // #3 at the read is s2 (s0 s1 s2 ...); live, it would be s1 (s0 n1 s1 s3 ...)
    run(&s, a, &["shape", "at", "#3", "f0", "hit", "--base", &v1]);
    let o = run(&s, a, &["shape", "get", "s2"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("f0=hit"));
    let o = run(&s, a, &["shape", "get", "s1"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("f0=later"));
}

/// The pack contract's byte-exact row (`test_support::carrier_judge`) on `deck-cost`, which
/// the open's file check leans on.
#[test]
fn deck_cost_passes_the_carrier_row() {
    let (a, b) = (deck(3), deck(40));
    let corpus: [(&str, &[u8]); 2] = [("small.deck", a.as_bytes()), ("larger.deck", b.as_bytes())];
    assert_eq!(
        carrier_judge(Deck::new(), &corpus),
        Ok(CarrierVerdict::ByteExact { files: 2 })
    );
}

#[test]
fn a_one_entry_read_visits_a_constant_number_of_entries() {
    row_read_visits().unwrap();
}

#[test]
fn a_one_entry_verb_visits_a_constant_number_of_entries() {
    row_verb_visits().unwrap();
}

#[test]
fn a_one_element_list_insert_move_and_delete_visit_a_constant_number_of_entries() {
    row_list_visits().unwrap();
}

#[test]
fn an_untouched_open_renders_nothing() {
    row_open_renders().unwrap();
}

/// The plants, each caught by its row, by name; the open row stays green under the read's.
#[test]
fn the_plants_are_caught_by_name() {
    use differential::{with_keyed_mutant, KeyedMutant};
    let e = with_keyed_mutant(KeyedMutant::ReadVisitsWholeDoc, row_read_visits)
        .expect_err("ReadVisitsWholeDoc is caught");
    assert!(e.starts_with("read visits:"), "{e}");
    with_keyed_mutant(KeyedMutant::ReadVisitsWholeDoc, row_open_renders)
        .expect("ReadVisitsWholeDoc leaves the open alone");
    let e = with_keyed_mutant(KeyedMutant::OpenRendersToCompare, row_open_renders)
        .expect_err("OpenRendersToCompare is caught");
    assert!(e.starts_with("open renders:"), "{e}");
    let e = with_keyed_mutant(KeyedMutant::ListReReadsRoot, row_list_visits)
        .expect_err("ListReReadsRoot is caught");
    assert!(e.starts_with("list visits:"), "{e}");
    with_keyed_mutant(KeyedMutant::ListReReadsRoot, row_verb_visits)
        .expect("ListReReadsRoot leaves a field set alone");
}
