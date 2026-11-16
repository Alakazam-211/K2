//! The carrier bytes: for a pack that models its format only in part, the kit keeps the bytes
//! it last read from the file or wrote to it (the "carrier", a blob in the blob store with a
//! count the store's lifecycle tracks) and hands them to `Pack::encode_from`, which splices the
//! doc's content into them.
//!
//! - `encode_from` gets exactly the bytes last read or written: after the open, after each kit
//!   write, after an outside save (from the save on, before anything renders the doc that took
//!   it); a conflict copy of the kit's version is rendered from the carrier, byte for byte.
//! - The carrier survives a reload (from the store, without the file; and across an app's save
//!   while the doc wasn't loaded).
//! - A history purge and a participant erasure reach the carrier blob: a byte scan of the store
//!   file and the blob folder finds no erased byte afterwards.
//! - N−1 in process: a store with no carrier (as a previous release leaves it) and one whose
//!   carrier an older write left stale both open, and the hook gets the file's bytes.
//! - An untouched live doc writes back byte for byte.
//! - A pack that doesn't opt in sees no change: no hook call, no blob.
//!
//! Plants (`KeyedMutant`): "carrier stale" (a kit write doesn't become the carrier), "purge
//! keeps carrier" (a replaced carrier is left for GC) and "settle before carrier" (an outside
//! save becomes the carrier only after the doc that took it was rendered), each caught by its
//! rows, by name.
//!
//! The test pack, `deck-carrier`: `deck-lite`'s file (one line per shape: its key, then
//! `\t<field>=<value>` per field) with **opaque lines**, any line starting with `#`, which it
//! never models. `encode` drops them and writes fields in key order; `encode_from` splices: a
//! shape whose fields are what the carrier's line says keeps that line verbatim, a changed or new
//! one gets the canonical line, and each `#` line stays after the shape line it followed. It
//! records every carrier it is handed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::{self, KeyedMutant};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::test_support::{carrier_judge, CarrierVerdict};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, DiskCheck, DiskState, EditOptions, Host, Pack,
    PackContent, PackError, Participant, ParticipantKind, PurgeRequest, PurgeScope, Reconciled,
    Rights, Session, SessionManager, Stay, View,
};
use k2_mpc_base::store::{scan_file, DocKey, DocStore};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError};

// ---------- the test pack ----------

const SHAPES: &str = "shapes";
const FIELDS: &str = "fields";

const DECK: Schema = Schema {
    name: "deck-carrier",
    roots: &[(SHAPES, Kind::MovableList), (FIELDS, Kind::Map)],
};

/// The pack. `carrier`: whether it opts in (`Pack::wants_carrier`).
struct Deck {
    carrier: bool,
    seen: Mutex<Vec<Vec<u8>>>,
    /// Every `encode` and `encode_from` call (a test-only count, never a timing).
    encodes: AtomicU64,
    /// While set, `encode` and `encode_from` fail (a file the pack can't render).
    failing: std::sync::atomic::AtomicBool,
}

impl Deck {
    fn new(carrier: bool) -> Arc<Self> {
        Arc::new(Self {
            carrier,
            seen: Mutex::new(Vec::new()),
            encodes: AtomicU64::new(0),
            failing: std::sync::atomic::AtomicBool::new(false),
        })
    }
    fn encodes(&self) -> u64 {
        self.encodes.load(Ordering::SeqCst)
    }
    /// The carrier `encode_from` was last handed.
    fn last(&self) -> Option<Vec<u8>> {
        self.seen.lock().unwrap().last().cloned()
    }
    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

/// A shape line: its key, its fields in file order, and the line itself (newline included).
struct ShapeLine<'a> {
    key: Key,
    fields: BTreeMap<String, String>,
    raw: &'a str,
}

/// A file: the opaque lines before the first shape, then each shape with the opaque lines after
/// it.
type Parsed<'a> = (Vec<&'a str>, Vec<(ShapeLine<'a>, Vec<&'a str>)>);

fn bad(message: String) -> Declined {
    Declined {
        code: "malformed",
        message,
    }
}

fn parse(b: &[u8]) -> Result<Parsed<'_>, Declined> {
    let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
    let mut lead = Vec::new();
    let mut shapes: Vec<(ShapeLine<'_>, Vec<&str>)> = Vec::new();
    for raw in s.split_inclusive('\n') {
        if raw.starts_with('#') {
            match shapes.last_mut() {
                Some((_, after)) => after.push(raw),
                None => lead.push(raw),
            }
            continue;
        }
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
        shapes.push((ShapeLine { key, fields, raw }, Vec::new()));
    }
    Ok((lead, shapes))
}

/// Each shape's key and fields, in order.
type Shapes = Vec<(Key, BTreeMap<String, String>)>;

/// The doc's shapes, and each shape's fields, from its content.
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
        let (_, shapes) = parse(b)?;
        let mut list = Vec::new();
        let mut fields = BTreeMap::new();
        for (s, _) in shapes {
            for (f, v) in &s.fields {
                fields.insert(format!("{}:{f}", s.key.as_str()), Value::str(v));
            }
            list.push((s.key, Value::Null));
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
        self.encodes.fetch_add(1, Ordering::SeqCst);
        if self.failing.load(Ordering::SeqCst) {
            return Err(PackError("a planted failure".into()));
        }
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
        self.carrier
    }
    fn encode_from(
        &self,
        carrier: &[u8],
        c: &[(String, Neutral)],
        _: &[u8],
    ) -> Result<Vec<u8>, PackError> {
        self.encodes.fetch_add(1, Ordering::SeqCst);
        if self.failing.load(Ordering::SeqCst) {
            return Err(PackError("a planted failure".into()));
        }
        self.seen.lock().unwrap().push(carrier.to_vec());
        let (lead, old) = parse(carrier).map_err(|d| PackError(d.message))?;
        let mut out: String = lead.concat();
        for (k, fields) in shapes_of(c)? {
            match old.iter().find(|(s, _)| s.key == k) {
                Some((s, after)) => {
                    if s.fields == fields {
                        out.push_str(s.raw);
                    } else {
                        out.push_str(&canonical(&k, &fields));
                    }
                    out.push_str(&after.concat());
                }
                None => out.push_str(&canonical(&k, &fields)),
            }
        }
        Ok(out.into_bytes())
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], _: &[u8]) -> Result<Reconciled, Declined> {
        let new = self.decode(bytes)?.roots;
        let (Neutral::List(shapes), Neutral::Map(fields)) = (&new[0].1, &new[1].1) else {
            unreachable!("decode's roots")
        };
        let old: &[(Key, Value)] = synced.list(SHAPES).unwrap_or(&[]);
        let hidden = synced.hidden(SHAPES);
        let empty = BTreeMap::new();
        let old_fields = synced.map(FIELDS).unwrap_or(&empty);
        let old_keys: HashSet<&Key> = old.iter().map(|(k, _)| k).collect();
        let new_keys: HashSet<&Key> = shapes.iter().map(|(k, _)| k).collect();
        let mut list = Vec::new();
        for (k, _) in old.iter().filter(|(k, _)| !new_keys.contains(k)) {
            list.push(ListOp::Delete { key: k.clone() });
        }
        let mut prev: Option<Key> = None;
        for (k, _) in shapes {
            if !old_keys.contains(k) {
                if hidden.is_some_and(|h| h.contains_key(k)) {
                    list.push(ListOp::Restore { key: k.clone() });
                } else {
                    list.push(ListOp::Insert {
                        key: k.clone(),
                        at: prev.clone().map_or(Anchor::Start, Anchor::After),
                        value: Value::Null,
                    });
                }
            }
            prev = Some(k.clone());
        }
        let mut map = Vec::new();
        for (k, v) in fields {
            if old_fields.get(k) != Some(v) {
                map.push(MapOp::Set {
                    key: k.clone(),
                    value: v.clone(),
                });
            }
        }
        for k in old_fields.keys().filter(|k| !fields.contains_key(*k)) {
            map.push(MapOp::Delete { key: k.clone() });
        }
        let mut edits = Vec::new();
        if !list.is_empty() {
            edits.push(Edit::List {
                root: SHAPES.into(),
                ops: list,
            });
        }
        if !map.is_empty() {
            edits.push(Edit::Map {
                root: FIELDS.into(),
                ops: map,
            });
        }
        Ok(Reconciled {
            edits,
            codec: vec![1],
            ..Reconciled::default()
        })
    }
}

/// `shape list`: a structured read verb (it reads the keyed state): each shape's key, in order.
fn shape_list(_: &[String]) -> Result<StructEdit, ToolError> {
    Ok(Box::new(|ctx| {
        let mut out = String::new();
        for (k, _) in ctx.view().list(SHAPES).unwrap_or(&[]) {
            out.push_str(k.as_str());
            out.push('\n');
        }
        Ok(StructOutput::new(Vec::new(), out.into_bytes(), 0))
    }))
}

const VERBS: &[PackVerb] = &[PackVerb::structured_read(
    "shape list",
    "shape list",
    &[PersonAction::ReadAll],
    &[0],
    shape_list,
)];

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
}

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    dir: TmpDir,
    ws: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    blobs: Arc<BlobStore>,
    mgr: SessionManager,
    pack: Arc<Deck>,
}

const NAME: &str = "talk.deck";

fn fx(tag: &str, carrier: bool) -> Fx {
    fx_with(tag, carrier, |_| {})
}

fn fx_with(tag: &str, carrier: bool, set: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-live-carrier-{tag}-{}-{}",
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
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(p.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            p.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    set(&mut c);
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs.clone());
    Fx {
        dir: TmpDir(p),
        ws,
        host,
        store,
        blobs,
        mgr,
        pack: Deck::new(carrier),
    }
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
    fn write(&self, bytes: &str) {
        fs::write(self.path(), bytes).unwrap();
    }
    fn bytes(&self) -> String {
        fs::read_to_string(self.path()).unwrap()
    }
    fn open(&self) -> Arc<Session> {
        self.open_with(self.pack.clone())
    }
    fn open_with(&self, pack: Arc<Deck>) -> Arc<Session> {
        self.mgr
            .open_file(pack, &key(), &self.path(), &self.ws)
            .unwrap()
    }
    fn join(&self, s: &Session, pid: &str, kind: ParticipantKind, owner: bool) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::full().with_owner(owner));
        s.join(
            Participant::new(pid, pid, "#123456", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    fn agent(&self, s: &Session) -> ConnId {
        self.join(s, "bot", ParticipantKind::Agent, false)
    }
    fn owner(&self, s: &Session) -> ConnId {
        self.join(s, "own", ParticipantKind::Person, true)
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    /// A crash: a new manager on the same stores, the old one dropped without a write-back.
    fn restart(&mut self) {
        self.mgr = SessionManager::new(
            self.store.clone(),
            self.host.clone(),
            self.mgr.config().clone(),
        )
        .with_blob_store(self.blobs.clone());
    }
    fn last(&self) -> Option<String> {
        self.pack
            .last()
            .map(|b| String::from_utf8(b).expect("UTF-8"))
    }
    /// The carriers `encode_from` was handed from its `n`-th call on.
    fn handed_since(&self, n: usize) -> Vec<String> {
        self.pack.seen.lock().unwrap()[n..]
            .iter()
            .map(|b| String::from_utf8(b.clone()).expect("UTF-8"))
            .collect()
    }
    /// Findings of `needle` in every file under the blob folder.
    fn blob_findings(&self, needle: &str) -> usize {
        files_under(&self.dir.0.join("blobs"))
            .iter()
            .map(|p| scan_file(p, &[needle.as_bytes()]).unwrap().len())
            .sum()
    }
    /// Findings of `needle` in the store file.
    fn store_findings(&self, needle: &str) -> usize {
        self.store.scan_for(&[needle.as_bytes()]).unwrap().len()
    }
    fn blob_count(&self) -> u64 {
        self.blobs.stats().unwrap().blobs
    }
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// An app's save: written beside the file, then renamed over it.
fn app_save(path: &Path, bytes: &str) {
    let tmp = path.with_file_name(".app-save.tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn text(s: &Session, c: ConnId) -> String {
    String::from_utf8(s.read(c).unwrap().bytes).unwrap()
}

/// Set `shape`'s `field` as `c` (at the version it read, as an agent must).
fn set_field(s: &Session, c: ConnId, shape: &str, field: &str, value: &str) -> u64 {
    let mut opts = EditOptions::new();
    opts.base = Some(s.read(c).unwrap().version);
    s.edit_ops(
        c,
        vec![Edit::Map {
            root: FIELDS.into(),
            ops: vec![MapOp::Set {
                key: format!("{shape}:{field}"),
                value: Value::str(value),
            }],
        }],
        &opts,
    )
    .unwrap()
    .version
}

/// [`set_field`] with the base the status gives (no read of the file's bytes).
fn set_quiet(s: &Session, c: ConnId, shape: &str, field: &str, value: &str) -> u64 {
    let mut opts = EditOptions::new();
    opts.base = Some(s.status().unwrap().version);
    s.edit_ops(
        c,
        vec![Edit::Map {
            root: FIELDS.into(),
            ops: vec![MapOp::Set {
                key: format!("{shape}:{field}"),
                value: Value::str(value),
            }],
        }],
        &opts,
    )
    .unwrap()
    .version
}

fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {}", what()))
    }
}

/// The deck: opaque lines before, between and after the shapes, and s2's fields out of key order
/// (`encode` would write them sorted), so only a splice writes it back as it was.
const F0: &str =
    "# deck v1 (opaque)\ns1\ttitle=A\n# speaker note for s1\ns2\tz=9\ttitle=B\n# end\n";
/// `F0` with s1's title `A2`: only that line changes.
const F1: &str =
    "# deck v1 (opaque)\ns1\ttitle=A2\n# speaker note for s1\ns2\tz=9\ttitle=B\n# end\n";
/// `F1` with s2's title B2: that line is written in key order.
const F2: &str =
    "# deck v1 (opaque)\ns1\ttitle=A2\n# speaker note for s1\ns2\ttitle=B2\tz=9\n# end\n";

// ---------- encode_from gets exactly the bytes last read or written ----------

/// After the open the carrier is the file the kit read; after each kit write, the bytes it
/// wrote (with no read of the file in between); after an outside save, the save. The kit's
/// version an outside save keeps as a conflict copy is rendered from the carrier, byte for byte.
fn row_encode_from_gets_the_bytes_last_read_or_written() -> Result<(), String> {
    const ROW: &str = "row_encode_from_gets_the_bytes_last_read_or_written";
    let f = fx("last", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    let got = text(&s, bot);
    need(f.last().as_deref() == Some(F0), ROW, || {
        format!(
            "after the open, the carrier is the file read: {:?}",
            f.last()
        )
    })?;
    need(got == F0, ROW, || {
        format!("the read renders the file: {got:?}")
    })?;
    set_field(&s, bot, "s1", "title", "A2");
    need(s.flush(bot).unwrap().written, ROW, || {
        "control: the kit wrote the file".into()
    })?;
    need(f.bytes() == F1, ROW, || {
        format!("the write is a splice: {:?}", f.bytes())
    })?;
    text(&s, bot);
    need(f.last().as_deref() == Some(F1), ROW, || {
        format!(
            "after the kit's first write, the carrier is the bytes written: {:?}",
            f.last()
        )
    })?;
    set_field(&s, bot, "s2", "title", "B2");
    need(s.flush(bot).unwrap().written, ROW, || {
        "control: the kit's second write".into()
    })?;
    need(f.bytes() == F2, ROW, || {
        format!("the second write is a splice: {:?}", f.bytes())
    })?;
    text(&s, bot);
    need(f.last().as_deref() == Some(F2), ROW, || {
        format!(
            "after the kit's second write, the carrier is the bytes written: {:?}",
            f.last()
        )
    })?;
    // an app's save over the kit's write keeps both: the kit's version is the carrier, untouched
    f.advance(1_000);
    const SAVE: &str =
        "# the app's header\ns1\ttitle=Person\n# speaker note for s1\ns2\ttitle=B2\tz=9\n";
    app_save(&f.path(), SAVE);
    let out = s.file_changed().unwrap();
    let DiskCheck::Conflict {
        copy: Some(copy), ..
    } = out
    else {
        return Err(format!("{ROW}: control: the save keeps both: {out:?}"));
    };
    let held = fs::read_to_string(&copy).unwrap();
    need(held == F2, ROW, || {
        format!("the conflict copy is the kit's version, from the carrier: {held:?}")
    })?;
    let got = text(&s, bot);
    need(f.last().as_deref() == Some(SAVE), ROW, || {
        format!(
            "after an outside save, the carrier is the save read: {:?}",
            f.last()
        )
    })?;
    need(got == SAVE, ROW, || {
        format!("the save is the live doc: {got:?}")
    })
}

#[test]
fn encode_from_gets_the_bytes_last_read_or_written() {
    row_encode_from_gets_the_bytes_last_read_or_written().unwrap();
}

/// The planted fault "carrier stale": the row goes red, by name.
#[test]
fn the_planted_carrier_stale_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::CarrierStale, || {
        let name = "row_encode_from_gets_the_bytes_last_read_or_written";
        let e = row_encode_from_gets_the_bytes_last_read_or_written()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("after the kit's first write"), "{e}");
    });
}

// ---------- an outside save is the carrier before anything renders ----------

/// An app's save (a header of its own, s1's title changed) on a doc with no unwritten change is
/// the carrier before the kit renders the doc that took it: every rendering from the save on is
/// handed the save, never the bytes before it; the doc is clean, the file read is the save, and
/// nothing is written.
fn row_an_outside_save_is_the_carrier_before_any_rendering() -> Result<(), String> {
    const ROW: &str = "row_an_outside_save_is_the_carrier_before_any_rendering";
    let f = fx("save-first", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    f.advance(1_000);
    const SAVE: &str =
        "# the app's header\ns1\ttitle=Person\n# speaker note for s1\ns2\tz=9\ttitle=B\n# end\n";
    app_save(&f.path(), SAVE);
    let from = f.pack.calls();
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    need(matches!(out, DiskCheck::Ingested { .. }), ROW, || {
        format!("control: the save is taken in: {out:?}")
    })?;
    let handed = f.handed_since(from);
    need(handed.iter().all(|c| c == SAVE), ROW, || {
        format!("a rendering after the save was handed another carrier: {handed:?}")
    })?;
    let disk = s.status().map(|x| x.disk);
    need(disk == Ok(Some(DiskState::Clean)), ROW, || {
        format!("the doc holds the save, clean: {disk:?}")
    })?;
    let got = text(&s, bot);
    need(got == SAVE, ROW, || {
        format!("the file read after the save is the save: {got:?}")
    })?;
    need(matches!(s.flush(bot), Ok(o) if !o.written), ROW, || {
        "nothing to write after the save".into()
    })
}

#[test]
fn an_outside_save_is_the_carrier_before_any_rendering() {
    row_an_outside_save_is_the_carrier_before_any_rendering().unwrap();
}

/// The planted fault "settle before carrier": the row goes red, by name.
#[test]
fn the_planted_settle_before_carrier_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::SettleBeforeCarrier, || {
        let name = "row_an_outside_save_is_the_carrier_before_any_rendering";
        let e = row_an_outside_save_is_the_carrier_before_any_rendering()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("handed another carrier"), "{e}");
    });
}

// ---------- the carrier survives a reload ----------

/// After a restart the carrier comes from the store: opened without its file, the hook gets the
/// kit's last write; and an app's save made while the doc wasn't loaded keeps the kit's version
/// as a copy rendered from it, byte for byte.
#[test]
fn the_carrier_survives_a_reload() {
    let mut f = fx("reload", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    set_field(&s, bot, "s1", "title", "A2");
    assert!(s.flush(bot).unwrap().written, "control: the kit wrote");
    assert_eq!(f.bytes(), F1);
    drop(s);
    f.restart();
    // without the file: the stored carrier
    let calls = f.pack.calls();
    let s = f.mgr.open(f.pack.clone(), &key(), None).unwrap();
    let bot = f.agent(&s);
    let got = text(&s, bot);
    assert!(f.pack.calls() > calls, "the read went through the hook");
    assert_eq!(
        f.last().as_deref(),
        Some(F1),
        "after a reload, the carrier is the kit's last write, from the store"
    );
    assert_eq!(got, F1);
    drop(s);
    f.restart();
    // an app saves over the kit's write while the doc isn't loaded
    f.advance(1_000);
    app_save(&f.path(), "s1\ttitle=Person\n");
    let s = f.open();
    let copy = f
        .host
        .events
        .lock()
        .unwrap()
        .iter()
        .find_map(|e| match e {
            AuditEvent::BlobConflict { copy, .. } => copy.clone(),
            _ => None,
        })
        .expect("the save over the kit's write keeps both");
    let held = fs::read_to_string(f.ws.join(&copy)).unwrap();
    assert_eq!(
        held, F1,
        "the copy is the kit's version, rendered from the stored carrier"
    );
    let bot = f.agent(&s);
    assert_eq!(text(&s, bot), "s1\ttitle=Person\n");
    assert_eq!(f.last().as_deref(), Some("s1\ttitle=Person\n"));
}

// ---------- purge and erasure reach the carrier ----------

const SECRET: &str = "Qz7Kx9Wm3pV8rT2nJ4hB6yL1";

/// The agent writes a secret into s1 and the kit writes the file (the carrier holds it); the
/// owner then overwrites it (not written yet) and purges with `scope`. Afterwards no byte of the
/// secret is in the store file or the blob folder, the file holds the purged doc, and the hook
/// is handed the file's bytes.
fn purge_reaches_the_carrier(row: &str, scope: PurgeScope) -> Result<(), String> {
    let f = fx("purge", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    let own = f.owner(&s);
    set_field(&s, bot, "s1", "title", SECRET);
    need(s.flush(bot).unwrap().written, row, || {
        "control: the kit wrote the secret".into()
    })?;
    need(f.blob_findings(SECRET) > 0, row, || {
        "control: the carrier in the blob folder holds the secret".into()
    })?;
    set_field(&s, own, "s1", "title", "clean");
    s.purge(own, PurgeRequest::new(scope))
        .map_err(|e| format!("{row}: the purge: {e}"))?;
    need(!f.bytes().contains(SECRET), row, || {
        "the file is written back without it".into()
    })?;
    let blobs = f.blob_findings(SECRET);
    need(blobs == 0, row, || {
        format!("the byte scan of the blob folder finds the secret: {blobs} findings")
    })?;
    let store = f.store_findings(SECRET);
    need(store == 0, row, || {
        format!("the byte scan of the store file finds the secret: {store} findings")
    })?;
    let got = text(&s, own);
    need(f.last().as_deref() == Some(f.bytes().as_str()), row, || {
        format!("after the purge the carrier is the file: {:?}", f.last())
    })?;
    need(got == f.bytes(), row, || {
        format!("the read is the file: {got:?}")
    })
}

fn row_a_history_purge_reaches_the_carrier() -> Result<(), String> {
    purge_reaches_the_carrier(
        "row_a_history_purge_reaches_the_carrier",
        PurgeScope::History,
    )
}

fn row_a_participant_erasure_reaches_the_carrier() -> Result<(), String> {
    purge_reaches_the_carrier(
        "row_a_participant_erasure_reaches_the_carrier",
        PurgeScope::Participant {
            pid: "bot".into(),
            pseudonym: "someone".into(),
        },
    )
}

#[test]
fn a_history_purge_reaches_the_carrier() {
    row_a_history_purge_reaches_the_carrier().unwrap();
}

#[test]
fn a_participant_erasure_reaches_the_carrier() {
    row_a_participant_erasure_reaches_the_carrier().unwrap();
}

/// The planted fault "purge keeps carrier": both purge rows go red at the blob folder's scan,
/// by name.
#[test]
fn the_planted_purge_keeps_carrier_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::PurgeKeepsCarrier, || {
        for (name, row) in [
            (
                "row_a_history_purge_reaches_the_carrier",
                row_a_history_purge_reaches_the_carrier as fn() -> Result<(), String>,
            ),
            (
                "row_a_participant_erasure_reaches_the_carrier",
                row_a_participant_erasure_reaches_the_carrier,
            ),
        ] {
            let e = row().expect_err(&format!("planted, {name} must fail"));
            assert!(e.starts_with(&format!("{name}: ")), "{e}");
            assert!(e.contains("the byte scan of the blob folder"), "{e}");
        }
    });
}

/// A doc delete gives the carrier's count back: past the grace, blob GC removes it.
#[test]
fn a_doc_delete_releases_the_carrier() {
    let f = fx("delete", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    text(&s, bot);
    assert_eq!(f.blob_count(), 1, "control: the carrier is a blob");
    assert_eq!(f.blobs.gc().unwrap().removed, 0, "control: GC keeps it");
    drop(s);
    f.mgr.delete_doc(&key()).unwrap();
    f.advance(2 * 60 * 60_000);
    f.blobs.gc().unwrap();
    assert_eq!(f.blob_count(), 0, "the delete gave the count back");
    assert_eq!(f.blob_findings("speaker note"), 0);
}

// ---------- N−1 in process ----------

/// A store a previous release wrote has no carrier, and a previous release's write leaves the
/// carrier stale; both open here and the hook gets the file's bytes. In process, the previous
/// release is a session of the same pack name that doesn't opt in: it keeps no carrier and
/// leaves the stored one as it was (`tools/n1/n1.sh layout-carrier` runs the real one).
#[test]
fn n1_a_store_with_no_carrier_or_a_stale_one_opens() {
    let mut f = fx("n1", true);
    f.write(F0);
    // the older kit imports and writes: no carrier
    let old = Deck::new(false);
    let s = f.open_with(old.clone());
    let p = f.owner(&s);
    set_field(&s, p, "s1", "title", "A2");
    s.flush(p).unwrap();
    let older = f.bytes();
    assert_eq!(older, "s1\ttitle=A2\ns2\ttitle=B\tz=9\n", "control: encode");
    assert_eq!(old.calls(), 0);
    assert_eq!(f.blob_count(), 0, "control: no carrier");
    drop(s);
    f.restart();
    // this kit: the carrier is the file it reads
    let s = f.open();
    let p = f.owner(&s);
    assert_eq!(text(&s, p), older);
    assert_eq!(f.last().as_deref(), Some(older.as_str()));
    assert_eq!(f.blob_count(), 1, "the carrier is kept");
    set_field(&s, p, "s2", "title", "B2");
    assert!(s.flush(p).unwrap().written);
    let ours = f.bytes();
    assert_eq!(ours, "s1\ttitle=A2\ns2\ttitle=B2\tz=9\n");
    drop(s);
    f.restart();
    // the older kit writes again, leaving the stored carrier as it was
    let s = f.open_with(old.clone());
    let p = f.owner(&s);
    set_field(&s, p, "s1", "title", "A3");
    s.flush(p).unwrap();
    let older2 = f.bytes();
    assert_eq!(older2, "s1\ttitle=A3\ns2\ttitle=B2\tz=9\n");
    drop(s);
    f.restart();
    // this kit: the stale carrier (`ours`) is dropped, the file read is the carrier
    let s = f.open();
    let p = f.owner(&s);
    assert_eq!(text(&s, p), older2);
    assert_eq!(f.last().as_deref(), Some(older2.as_str()));
    assert_eq!(f.blob_count(), 1, "the stale carrier's blob is gone");
    // and without its file after a restart: the carrier stored is the one read
    drop(s);
    f.restart();
    let s = f.mgr.open(f.pack.clone(), &key(), None).unwrap();
    let p = f.owner(&s);
    assert_eq!(text(&s, p), older2);
    assert_eq!(f.last().as_deref(), Some(older2.as_str()));
}

// ---------- untouched is byte for byte ----------

/// An untouched live doc writes back byte for byte: opened and flushed, the file is as it was
/// (opaque lines and field order kept); a change put back leaves it so; a change to one shape
/// changes only that shape's line.
fn row_an_untouched_live_doc_writes_back_byte_for_byte() -> Result<(), String> {
    const ROW: &str = "row_an_untouched_live_doc_writes_back_byte_for_byte";
    let f = fx("f5", true);
    f.write(F0);
    let s = f.open();
    let p = f.owner(&s);
    s.flush(p).unwrap();
    need(f.bytes() == F0, ROW, || {
        format!("an untouched doc's flush leaves the file: {:?}", f.bytes())
    })?;
    let got = text(&s, p);
    need(got == F0, ROW, || format!("the read is the file: {got:?}"))?;
    set_field(&s, p, "s1", "title", "A2");
    set_field(&s, p, "s1", "title", "A");
    s.flush(p).unwrap();
    need(f.bytes() == F0, ROW, || {
        format!("a change put back leaves the file: {:?}", f.bytes())
    })?;
    set_field(&s, p, "s1", "title", "A2");
    s.flush(p).unwrap();
    need(f.bytes() == F1, ROW, || {
        format!("one shape's change changes its line only: {:?}", f.bytes())
    })
}

#[test]
fn an_untouched_live_doc_writes_back_byte_for_byte() {
    row_an_untouched_live_doc_writes_back_byte_for_byte().unwrap();
}

// ---------- the pack contract's byte-exact row (`test_support::carrier_judge`) ----------

/// A carrier pack whose splice canonicalises: its `encode_from` is `encode`, which drops the
/// opaque lines, so an untouched doc doesn't write back byte for byte.
struct Canonical(Arc<Deck>);

impl Pack for Canonical {
    fn schema(&self) -> Schema {
        self.0.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        self.0.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        self.0.encode(c, codec)
    }
    fn wants_carrier(&self) -> bool {
        true
    }
    fn encode_from(
        &self,
        _: &[u8],
        c: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, PackError> {
        self.0.encode(c, codec)
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], codec: &[u8]) -> Result<Reconciled, Declined> {
        self.0.reconcile(synced, bytes, codec)
    }
}

/// The kit's row for the pack contract: `deck-carrier` writes back byte for byte (the hook,
/// through the kit, after a restart) over its corpus.
#[test]
fn the_carrier_judge_passes_a_byte_exact_pack() {
    let corpus: [(&str, &[u8]); 3] = [
        ("a.deck", F0.as_bytes()),
        ("b.deck", F1.as_bytes()),
        ("c.deck", F2.as_bytes()),
    ];
    assert_eq!(
        carrier_judge(Deck::new(true), &corpus),
        Ok(CarrierVerdict::ByteExact { files: 3 })
    );
}

/// A pack that doesn't keep a carrier: the row doesn't apply, nothing is checked.
#[test]
fn the_carrier_judge_skips_a_pack_without_a_carrier() {
    assert_eq!(
        carrier_judge(Deck::new(false), &[("a.deck", F0.as_bytes())]),
        Ok(CarrierVerdict::NotACarrierPack)
    );
}

/// The row's negative: a splice that canonicalises fails it at the hook and through the kit,
/// naming the file and the leg.
#[test]
fn the_carrier_judge_catches_a_splice_that_canonicalises() {
    let e = carrier_judge(
        Arc::new(Canonical(Deck::new(true))),
        &[("a.deck", F0.as_bytes())],
    )
    .expect_err("the opaque lines go");
    assert!(
        e.iter()
            .any(|l| l.starts_with("carrier row: a.deck: the hook:")),
        "{e:?}"
    );
    assert!(
        e.iter()
            .any(|l| l.starts_with("carrier row: a.deck: opened live:")),
        "{e:?}"
    );
}

// ---------- packs that don't opt in ----------

/// A pack that doesn't opt in: no hook call, no blob, the file written with `encode` as before.
#[test]
fn packs_that_dont_opt_in_see_no_change() {
    let mut f = fx("no-opt-in", false);
    f.write(F0);
    let s = f.open();
    let p = f.owner(&s);
    assert_eq!(
        text(&s, p),
        "s1\ttitle=A\ns2\ttitle=B\tz=9\n",
        "encode, as before"
    );
    set_field(&s, p, "s1", "title", "A2");
    assert!(s.flush(p).unwrap().written);
    assert_eq!(f.bytes(), "s1\ttitle=A2\ns2\ttitle=B\tz=9\n");
    f.advance(1_000);
    app_save(&f.path(), "# a header\ns1\ttitle=X\n");
    s.file_changed().unwrap();
    text(&s, p);
    drop(s);
    f.restart();
    let s = f.open();
    let p = f.owner(&s);
    text(&s, p);
    assert_eq!(f.pack.calls(), 0, "encode_from is never called");
    assert_eq!(f.blob_count(), 0, "no carrier is kept");
}

// ---------- conflict copies of a pack that keeps a carrier ----------

/// The kit's write of `s1`'s title `value` (flushed), then an app's save from the buffer it
/// opened before (s2's title changed): both kept, the copy beside the file. Returns the copy.
fn copy_after_a_stale_save(f: &Fx, s: &Session, bot: ConnId, value: &str) -> PathBuf {
    set_field(s, bot, "s1", "title", value);
    assert!(s.flush(bot).unwrap().written, "control: the kit wrote");
    f.advance(1_000);
    app_save(&f.path(), F0.replace("title=B", "title=Person").as_str());
    let DiskCheck::Conflict {
        copy: Some(copy), ..
    } = s.file_changed().unwrap()
    else {
        panic!("control: the save keeps both")
    };
    copy
}

/// `tick` removes an untouched conflict copy of a pack that keeps a carrier after its keep:
/// it compares the copy with what the kit wrote (the kept version spliced into its carrier),
/// not with `encode`. Red before: the copy (with its opaque lines) never matched and stayed.
fn row_tick_removes_an_untouched_copy_of_a_carrier_pack() -> Result<(), String> {
    const ROW: &str = "row_tick_removes_an_untouched_copy_of_a_carrier_pack";
    const KEEP: u64 = 600_000;
    let f = fx_with("copy-tick", true, |c| c.conflict_copy_keep_ms = KEEP);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    let copy = copy_after_a_stale_save(&f, &s, bot, "A2");
    let held = fs::read_to_string(&copy).unwrap();
    need(held == F1, ROW, || {
        format!("control: the copy is the kit's write: {held:?}")
    })?;
    f.advance(KEEP + 60_000);
    let rep = f.mgr.tick().unwrap();
    need(rep.errors.is_empty(), ROW, || {
        format!("tick errors: {:?}", rep.errors)
    })?;
    need(!copy.exists(), ROW, || {
        format!(
            "the untouched copy is removed after its keep: {}",
            copy.display()
        )
    })?;
    need(
        rep.conflict_copies_removed.iter().any(|(_, p)| *p == copy),
        ROW,
        || format!("the tick reports it: {:?}", rep.conflict_copies_removed),
    )
}

#[test]
fn tick_removes_an_untouched_copy_of_a_carrier_pack() {
    row_tick_removes_an_untouched_copy_of_a_carrier_pack().unwrap();
}

/// A purge removes a conflict copy holding content it erased, for a pack that keeps a carrier as
/// for any other. Red before: the copy never matched its kept version's `encode`, so it was
/// left and only reported.
fn row_a_purge_removes_a_copy_holding_erased_content() -> Result<(), String> {
    const ROW: &str = "row_a_purge_removes_a_copy_holding_erased_content";
    let f = fx("copy-purge", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    let own = f.owner(&s);
    let copy = copy_after_a_stale_save(&f, &s, bot, SECRET);
    need(
        fs::read_to_string(&copy).unwrap().contains(SECRET),
        ROW,
        || "control: the copy holds the secret".into(),
    )?;
    let out = s
        .purge(own, PurgeRequest::new(PurgeScope::History))
        .map_err(|e| format!("{ROW}: the purge: {e}"))?;
    need(!copy.exists(), ROW, || {
        format!(
            "the copy holding erased content is removed: {}",
            copy.display()
        )
    })?;
    need(out.counts.conflict_copies == 1, ROW, || {
        format!("the purge counts it: {:?}", out.counts)
    })?;
    let left = f.blob_findings(SECRET) + f.store_findings(SECRET);
    need(left == 0, ROW, || {
        format!("the byte scan finds the secret: {left} findings")
    })
}

#[test]
fn a_purge_removes_a_copy_holding_erased_content() {
    row_a_purge_removes_a_copy_holding_erased_content().unwrap();
}

/// The planted fault "copy compared with encode": both copy rows go red, by name.
#[test]
fn the_planted_copy_compared_with_encode_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::CopyComparedWithEncode, || {
        for (name, row) in [
            (
                "row_tick_removes_an_untouched_copy_of_a_carrier_pack",
                row_tick_removes_an_untouched_copy_of_a_carrier_pack as fn() -> Result<(), String>,
            ),
            (
                "row_a_purge_removes_a_copy_holding_erased_content",
                row_a_purge_removes_a_copy_holding_erased_content,
            ),
        ] {
            let e = row().expect_err(&format!("planted, {name} must fail"));
            assert!(e.starts_with(&format!("{name}: ")), "{e}");
        }
    });
}

// ---------- a version's file ----------

/// `file_at` of a version of a pack that keeps a carrier is that version's file byte for byte,
/// after later edits and an app's save that changed the opaque lines: the version's content
/// spliced into the carrier as of that version. Red before: `encode` (no opaque lines).
fn row_file_at_gives_that_versions_file_byte_for_byte() -> Result<(), String> {
    const ROW: &str = "row_file_at_gives_that_versions_file_byte_for_byte";
    let f = fx("file-at", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    set_field(&s, bot, "s1", "title", "A2");
    need(s.flush(bot).unwrap().written, ROW, || {
        "control: the kit wrote".into()
    })?;
    let v = s
        .name_version(bot, "after A2", None)
        .map_err(|e| format!("{ROW}: naming the version: {e}"))?;
    set_field(&s, bot, "s2", "title", "B2");
    need(s.flush(bot).unwrap().written, ROW, || {
        "control: the kit's second write".into()
    })?;
    // an app's save that changes the opaque header too (it keeps both: the kit wrote since)
    f.advance(1_000);
    app_save(&f.path(), &F2.replace("# deck v1", "# deck v2"));
    s.file_changed().unwrap();
    let got = s
        .file_at(bot, v)
        .map(|b| b.bytes)
        .map_err(|e| format!("{ROW}: file_at: {e}"))?;
    need(got == F1.as_bytes(), ROW, || {
        format!(
            "file_at is the version's file: {:?}",
            String::from_utf8_lossy(&got)
        )
    })
}

#[test]
fn file_at_gives_that_versions_file_byte_for_byte() {
    row_file_at_gives_that_versions_file_byte_for_byte().unwrap();
}

/// The planted fault "file_at uses encode": the row goes red, by name.
#[test]
fn the_planted_file_at_uses_encode_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::FileAtUsesEncode, || {
        let name = "row_file_at_gives_that_versions_file_byte_for_byte";
        let e = row_file_at_gives_that_versions_file_byte_for_byte()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("file_at is the version's file"), "{e}");
    });
}

/// A purge of a version reaches its carrier: the version that held the secret keeps the file's
/// bytes as of that version (a blob), and purging the version removes them at once. Red before:
/// no version kept a carrier.
#[test]
fn a_version_purge_reaches_the_versions_carrier() {
    let f = fx("version-purge", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    let own = f.owner(&s);
    set_field(&s, bot, "s1", "title", SECRET);
    assert!(s.flush(bot).unwrap().written);
    let v = s.name_version(own, "with the secret", None).unwrap();
    set_field(&s, own, "s1", "title", "clean");
    assert!(s.flush(own).unwrap().written);
    assert!(
        f.blob_findings(SECRET) > 0,
        "the version's carrier holds the secret in the blob folder"
    );
    let file = s.file_at(own, v).unwrap().bytes;
    assert!(String::from_utf8(file).unwrap().contains(SECRET));
    s.purge(
        own,
        PurgeRequest::new(PurgeScope::Versions { from: v, to: v }),
    )
    .unwrap();
    assert_eq!(
        f.blob_findings(SECRET),
        0,
        "the purge removed the version's carrier"
    );
}

/// A version with no carrier (one a previous release cut, or this build before the doc had a
/// carrier) is refused typed (`MPC-E208`, exit 4), never `encode`'s partial file. In process, the
/// previous release is a session of the same pack name that doesn't opt in.
#[test]
fn file_at_of_a_version_with_no_carrier_is_refused_typed() {
    let mut f = fx("no-carrier", true);
    f.write(F0);
    let old = Deck::new(false);
    let s = f.open_with(old);
    let p = f.owner(&s);
    set_field(&s, p, "s1", "title", "A2");
    let v = s.name_version(p, "older", None).unwrap();
    set_field(&s, p, "s2", "title", "B2");
    s.flush(p).unwrap();
    drop(s);
    f.restart();
    let s = f.open();
    let p = f.owner(&s);
    let Err(e) = s.file_at(p, v) else {
        panic!("a version with no carrier is refused");
    };
    let rep = e.report();
    assert_eq!(rep.id(), "MPC-E208", "{e}");
    assert_eq!(rep.exit(), 4);
}

// ---------- the removal race ----------

/// A put of the same bytes, not yet referenced, survives the carrier's replacement: the
/// replaced carrier is removed at once only when no put is waiting for its reference. Driven
/// in order: the carrier `F1` is held by the live doc alone; another writer puts `F1`; the kit
/// writes again (the carrier `F1` is released and would be removed); the other writer takes its
/// reference. Red before: that reference failed (`NoBlob`), the bytes gone.
fn row_a_waiting_put_of_the_same_bytes_survives_the_carriers_replacement() -> Result<(), String> {
    const ROW: &str = "row_a_waiting_put_of_the_same_bytes_survives_the_carriers_replacement";
    let f = fx("race", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    set_field(&s, bot, "s1", "title", "A2");
    need(
        s.flush(bot).unwrap().written && f.bytes() == F1,
        ROW,
        || "control: the kit wrote F1".into(),
    )?;
    // another writer's put of the same bytes, its reference not taken yet
    let theirs = f.blobs.put_bytes("ws", F1.as_bytes()).unwrap().id;
    // the kit replaces the carrier `F1`
    set_field(&s, bot, "s2", "title", "B2");
    need(s.flush(bot).unwrap().written, ROW, || {
        "control: the kit's second write".into()
    })?;
    // the other writer takes its reference
    f.blobs
        .retain("ws", &theirs)
        .map_err(|e| format!("{ROW}: the waiting put's reference failed: {e}"))?;
    let held = f.blobs.read_to_vec("ws", &theirs, 1 << 20);
    need(held.as_deref().ok() == Some(F1.as_bytes()), ROW, || {
        format!("the other writer's bytes are there: {held:?}")
    })
}

#[test]
fn a_waiting_put_of_the_same_bytes_survives_the_carriers_replacement() {
    row_a_waiting_put_of_the_same_bytes_survives_the_carriers_replacement().unwrap();
}

/// The planted fault "removal ignores waiting puts": the race row goes red, by name, at the
/// other writer's reference.
#[test]
fn the_planted_removal_ignores_waiting_puts_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::RemovalIgnoresWaitingPuts, || {
        let name = "row_a_waiting_put_of_the_same_bytes_survives_the_carriers_replacement";
        let e = row_a_waiting_put_of_the_same_bytes_survives_the_carriers_replacement()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("the waiting put's reference failed"), "{e}");
    });
}

// ---------- reads never render the file; one encode per sync point ----------

/// A structured read verb on a live doc makes no encode call (neither `encode` nor
/// `encode_from`): it reads the keyed state, and no "does the file hold the doc" check runs on
/// a read. Counted by the pack, not timed. Red before: every read rendered the whole file.
fn row_a_structured_read_makes_no_encode_call() -> Result<(), String> {
    const ROW: &str = "row_a_structured_read_makes_no_encode_call";
    let f = fx("read-encodes", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    let before = f.pack.encodes();
    for _ in 0..3 {
        let o = s
            .run_tool(bot, SHAPES, &["shape".into(), "list".into()], b"")
            .map_err(|e| format!("{ROW}: shape list: {e:?}"))?;
        need(o.stdout == b"s1\ns2\n", ROW, || {
            format!(
                "control: the read's output: {:?}",
                String::from_utf8_lossy(&o.stdout)
            )
        })?;
    }
    let n = f.pack.encodes() - before;
    need(n == 0, ROW, || {
        format!("three structured reads made {n} encode calls")
    })
}

#[test]
fn a_structured_read_makes_no_encode_call() {
    row_a_structured_read_makes_no_encode_call().unwrap();
}

/// The planted fault "read encodes": the read row goes red, by name.
#[test]
fn the_planted_read_encodes_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::ReadEncodes, || {
        let name = "row_a_structured_read_makes_no_encode_call";
        let e = row_a_structured_read_makes_no_encode_call()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("encode calls"), "{e}");
    });
}

/// A read that renders nothing is still told when the doc's file can't be rendered: the failure
/// the last rendering found (the sync's, here a flush's) is kept with the doc's version, and a
/// structured read at that version reports it without encoding; once the doc changes, the read
/// passes, and the next sync renders again.
#[test]
fn a_read_reports_a_render_failure_without_encoding() {
    let f = fx("render-fails", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    let list = |s: &Session| s.run_tool(bot, SHAPES, &["shape".into(), "list".into()], b"");
    f.pack.failing.store(true, Ordering::SeqCst);
    set_quiet(&s, bot, "s1", "title", "A2");
    let e = s.flush(bot).expect_err("the sync can't render the file");
    assert!(e.to_string().contains("a planted failure"), "{e}");
    let before = f.pack.encodes();
    let e = list(&s).expect_err("the read is told");
    assert!(format!("{e:?}").contains("a planted failure"), "{e:?}");
    assert_eq!(f.pack.encodes(), before, "the read encoded nothing");
    // the doc changes: no rendering at this version has failed, so the read passes
    f.pack.failing.store(false, Ordering::SeqCst);
    set_quiet(&s, bot, "s2", "title", "B2");
    let o = list(&s).unwrap();
    assert_eq!(o.stdout, b"s1\ns2\n");
    assert_eq!(f.pack.encodes(), before, "still no encode on a read");
    assert!(s.flush(bot).unwrap().written);
    assert_eq!(f.bytes(), F2);
}

/// N verbs between two sync points make at most one encode: the edits encode nothing, and the
/// write-back that follows renders the doc once (a flush's hash is the file's, not a second
/// rendering). The edits are durable in the store when each returns; the file follows at the
/// next write-back. Red before: the flush rendered twice.
#[test]
fn n_verbs_between_syncs_make_at_most_one_encode() {
    const N: usize = 20;
    let f = fx("one-encode", true);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    s.flush(bot).unwrap();
    let before = f.pack.encodes();
    for i in 0..N {
        let mut opts = EditOptions::new();
        opts.base = Some(s.status().unwrap().version);
        s.edit_ops(
            bot,
            vec![Edit::Map {
                root: FIELDS.into(),
                ops: vec![MapOp::Set {
                    key: "s1:title".into(),
                    value: Value::str(format!("A{i}")),
                }],
            }],
            &opts,
        )
        .unwrap();
    }
    assert_eq!(f.pack.encodes(), before, "the edits encode nothing");
    assert!(s.flush(bot).unwrap().written, "the sync writes the file");
    assert_eq!(
        f.pack.encodes() - before,
        1,
        "{N} verbs and their sync: one encode"
    );
    assert_eq!(
        f.bytes(),
        F1.replace("title=A2", &format!("title=A{}", N - 1))
    );
    // nothing changed since: the next sync encodes once more at most, and writes nothing
    let before = f.pack.encodes();
    assert!(!s.flush(bot).unwrap().written);
    assert!(f.pack.encodes() - before <= 1);
}

// ---------- N−1 against the previous release (tools/n1/n1.sh layout-carrier) ----------

const N1_T0: u64 = 1_000_000;
const HOUR: u64 = 60 * 60_000;

struct Clock(Arc<AtomicU64>);
impl Host for Clock {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn n1_dir() -> PathBuf {
    PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"))
}

fn n1_key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

/// The store, blob store and manager in `dir`, and its folder `dir/ws`.
fn n1_open(dir: &Path, now: u64) -> (Arc<BlobStore>, SessionManager, PathBuf) {
    fs::create_dir_all(dir.join("ws")).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let now = Arc::new(AtomicU64::new(now));
    let clock = now.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || clock.load(Ordering::SeqCst)),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(store, Arc::new(Clock(now)), Config::default())
        .with_blob_store(blobs.clone());
    (blobs, mgr, fs::canonicalize(dir.join("ws")).unwrap())
}

fn n1_person(s: &Session) -> ConnId {
    s.join(
        Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

/// `name`'s file after the producer: `F1` with s1's title `A2-<letter>` (three distinct carriers).
fn n1_written(name: &str) -> String {
    F1.replace("title=A2", &format!("title=A2-{}", &name[..1]))
}

/// Three live docs with a carrier each, for the previous release's consumer
/// (tools/n1/consumer-carrier): `a.deck` it writes, `b.deck` it leaves, `c.deck` it deletes.
#[test]
#[ignore = "tools/n1/n1.sh: writes the store for the previous release's consumer"]
fn n1_carrier_producer() {
    let dir = n1_dir();
    let (blobs, mgr, ws) = n1_open(&dir, N1_T0);
    for name in ["a.deck", "b.deck", "c.deck"] {
        fs::write(ws.join(name), F0).unwrap();
        let pack = Deck::new(true);
        let s = mgr
            .open_file(pack.clone(), &n1_key(name), &ws.join(name), &ws)
            .unwrap();
        let p = n1_person(&s);
        set_field(&s, p, "s1", "title", &format!("A2-{}", &name[..1]));
        assert!(s.flush(p).unwrap().written);
        assert_eq!(fs::read_to_string(ws.join(name)).unwrap(), n1_written(name));
        text(&s, p);
        assert_eq!(pack.last(), Some(n1_written(name).into_bytes()));
    }
    assert_eq!(
        blobs.stats().unwrap().blobs,
        4,
        "one carrier per doc, and the import's for the three import versions"
    );
    mgr.release().unwrap();
    println!("this build: three docs with a carrier each");
}

/// After the previous release's run: the carrier its write left stale is dropped and the file
/// it wrote is the carrier; the carrier it left alone is the stored one; the count its delete
/// left goes back at the open and GC removes the blob; and its own store, with no carrier, opens
/// with the file as the carrier. A previous release that knows the carrier tables
/// (`K2MB_N1_PREV` v0.15.0 and later) gave the deleted doc's count back itself, and its GC took
/// the blob: a plain round trip.
#[test]
#[ignore = "tools/n1/n1.sh: reads the store the previous release's consumer left"]
fn n1_carrier_after() {
    let prev_knows = std::env::var("K2MB_N1_PREV").is_ok_and(|v| {
        let n: Vec<u64> = v.trim().split('.').map(|x| x.parse().unwrap()).collect();
        assert_eq!(n.len(), 3, "K2MB_N1_PREV={v:?}: not MAJOR.MINOR.PATCH");
        (n[0], n[1], n[2]) >= (0, 15, 0)
    });
    let dir = n1_dir();
    let (blobs, mgr, ws) = n1_open(&dir, N1_T0 + 6 * HOUR);
    let id = |name: &str| {
        blobs
            .id_of("ws", &mut n1_written(name).as_bytes())
            .unwrap()
            .0
    };
    let refs = |name: &str| blobs.info("ws", &id(name)).unwrap().map(|x| x.1);
    if prev_knows {
        assert_eq!(
            refs("c.deck"),
            None,
            "the older kit's delete and GC took c's carrier"
        );
    } else {
        assert_eq!(refs("c.deck"), Some(0), "the open gave back c's count");
    }
    assert_eq!(refs("b.deck"), Some(1), "b's carrier is held");
    // b: the stored carrier, without the file
    let pack = Deck::new(true);
    let s = mgr.open(pack.clone(), &n1_key("b.deck"), None).unwrap();
    let p = n1_person(&s);
    assert_eq!(text(&s, p), n1_written("b.deck"));
    assert_eq!(pack.last(), Some(n1_written("b.deck").into_bytes()));
    assert_eq!(
        s.file_at(p, 1).unwrap().bytes,
        F0.as_bytes(),
        "the import version's file"
    );
    println!("b.deck: the stored carrier, untouched by the older kit; its version's file");
    // a: the older kit's write left the carrier stale: dropped, the file is the carrier
    let older = fs::read(ws.join("a.deck")).unwrap();
    assert_eq!(older, b"s1\ttitle=A2-a\ns2\ttitle=B-old\tz=9\n");
    let pack = Deck::new(true);
    let s = mgr
        .open_file(pack.clone(), &n1_key("a.deck"), &ws.join("a.deck"), &ws)
        .unwrap();
    let p = n1_person(&s);
    assert_eq!(text(&s, p).as_bytes(), older.as_slice());
    assert_eq!(pack.last(), Some(older.clone()));
    assert_eq!(refs("a.deck"), None, "the stale carrier's blob is gone");
    println!("a.deck: the stale carrier dropped, the older kit's write is the carrier");
    blobs.gc().unwrap();
    assert_eq!(refs("c.deck"), None, "GC took the deleted doc's carrier");
    assert_eq!(
        blobs.stats().unwrap().blobs,
        3,
        "a's and b's carriers, and the import versions'"
    );
    println!("c.deck: the count its delete left went back; GC removed the blob");
    mgr.release().unwrap();
    // the older kit's own store: no carrier; the file read is the carrier
    let (blobs, mgr, ws) = n1_open(&dir.join("old"), N1_T0 + 6 * HOUR);
    let older = fs::read(ws.join("d.deck")).unwrap();
    let pack = Deck::new(true);
    let s = mgr
        .open_file(pack.clone(), &n1_key("d.deck"), &ws.join("d.deck"), &ws)
        .unwrap();
    let p = n1_person(&s);
    assert_eq!(text(&s, p).as_bytes(), older.as_slice());
    assert_eq!(pack.last(), Some(older));
    assert_eq!(
        s.file_at(p, 1).map_err(|e| e.report().id()).err(),
        Some("MPC-E208"),
        "a version the older kit cut has no carrier"
    );
    set_field(&s, p, "s2", "title", "B2");
    assert!(s.flush(p).unwrap().written);
    assert_eq!(
        fs::read_to_string(ws.join("d.deck")).unwrap(),
        "s1\ttitle=D1\ns2\ttitle=B2\tz=9\n"
    );
    assert_eq!(blobs.stats().unwrap().blobs, 1, "its carrier is kept");
    mgr.release().unwrap();
    println!("old/d.deck: the older kit's store opens; the file read is the carrier; its version is refused typed");
    println!("N-1 CARRIER (this build) OK");
}
