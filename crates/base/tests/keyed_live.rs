//! A keyed doc that isn't a table (a deck, a board) on the live path, with a file behind it:
//! the guards it needs before any such type goes live.
//!
//! - **The size limit** (`Config::keyed_max_entries`): a file over it is declined
//!   `live_too_large` (`MPC-E415`) and still opens, at tier 0; a file at it opens live; a
//!   change that would grow the doc past it is refused (`AtLimit`), and an outside save past
//!   it declines the file. The planted fault "keyed limit not applied" turns each of those
//!   rows red, by name.
//! - **The outside-save rule** (the review's B2; tier 0's since v0.14.0): when the kit wrote the
//!   file since the last outside save, an app's save that differs keeps both. The save becomes
//!   the live doc; the kit's version is kept as a version and a conflict copy, with one
//!   `BlobConflict`, also across a restart and after a merge's write-back. With no kit write
//!   since, the save applies plainly; a save of exactly the kit's write is nothing; tables keep
//!   their own merge. The planted fault "keyed outside save applied silently" turns the
//!   stale-save rows red, by name.
//!
//! The test pack, `deck-lite`: a MovableList `shapes` and a Map `fields` keyed
//! `<shape>:<field>`. The file is one line per shape: its key, then `\t<field>=<value>` per
//! field in field order. Its `reconcile` pairs shapes by key (new keys inserted after the
//! shape before them, missing ones deleted) and fields by map key.
//!
//! Each row is a function returning `Err("<row name>: ...")` when it fails, so the planted
//! fault's test can show which row caught it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod grid_pack;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::{self, KeyedMutant};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::errors::{ErrorReport, Refusal, Value as ErrorValue};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, ConnId, Declined, DiskCheck, EditOptions, Host, LimitSource,
    NotReached, Pack, PackContent, PackError, Participant, ParticipantKind, PurgeRequest,
    PurgeScope, Reconciled, Rights, Session, SessionError, SessionManager, Stay, View,
};
use k2_mpc_base::store::{DocKey, DocStore};

// ---------- the test pack ----------

const SHAPES: &str = "shapes";
const FIELDS: &str = "fields";

struct Deck;

const DECK: Schema = Schema {
    name: "deck-lite",
    roots: &[(SHAPES, Kind::MovableList), (FIELDS, Kind::Map)],
};

type Parsed = (Vec<(Key, Value)>, BTreeMap<String, Value>);

fn parse(b: &[u8]) -> Result<Parsed, Declined> {
    let bad = |message: String| Declined {
        code: "malformed",
        message,
    };
    let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
    let mut shapes = Vec::new();
    let mut fields = BTreeMap::new();
    for line in s.lines() {
        let mut parts = line.split('\t');
        let k = parts.next().unwrap_or("");
        let key = Key::new(k).map_err(|e| bad(e.to_string()))?;
        for p in parts {
            let (f, v) = p
                .split_once('=')
                .ok_or_else(|| bad(format!("no = in {p:?}")))?;
            fields.insert(format!("{k}:{f}"), Value::str(v));
        }
        shapes.push((key, Value::Null));
    }
    Ok((shapes, fields))
}

impl Pack for Deck {
    fn schema(&self) -> Schema {
        DECK
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (shapes, fields) = parse(b)?;
        Ok(PackContent {
            roots: vec![
                (SHAPES.into(), Neutral::List(shapes)),
                (FIELDS.into(), Neutral::Map(fields)),
            ],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let mut shapes: &[(Key, Value)] = &[];
        let mut fields = &BTreeMap::new();
        for (r, n) in c {
            match n {
                Neutral::List(l) if r == SHAPES => shapes = l,
                Neutral::Map(m) if r == FIELDS => fields = m,
                _ => return Err(PackError(format!("unexpected root {r}"))),
            }
        }
        let mut out = String::new();
        for (k, _) in shapes {
            out.push_str(k.as_str());
            let prefix = format!("{}:", k.as_str());
            for (fk, v) in fields.range(prefix.clone()..) {
                let Some(f) = fk.strip_prefix(&prefix) else {
                    break;
                };
                out.push('\t');
                out.push_str(f);
                out.push('=');
                out.push_str(v.as_str().unwrap_or(""));
            }
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], _: &[u8]) -> Result<Reconciled, Declined> {
        let (shapes, fields) = parse(bytes)?;
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
        for (k, _) in &shapes {
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
        for (k, v) in &fields {
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

// ---------- harness ----------

type CopyNamer = fn(&str, u64, &str) -> Option<String>;

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    events: Mutex<Vec<AuditEvent>>,
    copy_names: Mutex<Option<CopyNamer>>,
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
    fn conflict_copy_name(&self, file_name: &str, at_ms: u64, who: &str) -> Option<String> {
        (*self.copy_names.lock().unwrap()).and_then(|f| f(file_name, at_ms, who))
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
    store: Arc<DocStore>,
    blobs: Arc<BlobStore>,
    mgr: SessionManager,
}

const NAME: &str = "talk.deck";

fn fx(tag: &str) -> Fx {
    fx_with(tag, |_| {})
}

fn fx_with(tag: &str, set: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-keyed-live-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        // 2026-10-27 14:30:00 UTC, so conflict-copy names are fixed
        now: AtomicU64::new(1_793_111_400_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
        copy_names: Mutex::new(None),
    });
    let store = Arc::new(DocStore::open(p.join("docs.redb")).unwrap());
    // a blob store: a declined file falls back to tier 0
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
        _dir: TmpDir(p),
        ws,
        host,
        store,
        blobs,
        mgr,
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
    fn try_open(&self) -> Result<Arc<Session>, SessionError> {
        self.mgr
            .open_file(Arc::new(Deck), &key(), &self.path(), &self.ws)
    }
    fn open(&self) -> Arc<Session> {
        self.try_open().unwrap()
    }
    fn join(&self, s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::full());
        s.join(
            Participant::new(pid, pid, "#123456", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    fn person(&self, s: &Session) -> ConnId {
        self.join(s, "ana", ParticipantKind::Person)
    }
    fn agent(&self, s: &Session) -> ConnId {
        self.join(s, "bot", ParticipantKind::Agent)
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
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
}

/// An app's save: written beside the file, then renamed over it.
fn app_save(path: &Path, bytes: &str) {
    let tmp = path.with_file_name(".app-save.tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn text(s: &Session, c: ConnId) -> String {
    String::from_utf8(s.read(c).unwrap().bytes).unwrap()
}

/// Set `shape`'s `field` as `c` (an agent passes the version it read as its base).
fn set_field(
    s: &Session,
    c: ConnId,
    shape: &str,
    field: &str,
    value: &str,
) -> Result<u64, SessionError> {
    let mut opts = EditOptions::new();
    opts.base = Some(s.read(c)?.version);
    let r = s.edit_ops(
        c,
        vec![Edit::Map {
            root: FIELDS.into(),
            ops: vec![MapOp::Set {
                key: format!("{shape}:{field}"),
                value: Value::str(value),
            }],
        }],
        &opts,
    )?;
    Ok(r.version)
}

/// Insert a shape `key` with one field, at the end, as `c`.
fn add_shape(s: &Session, c: ConnId, key: &str, title: &str) -> Result<u64, SessionError> {
    let mut opts = EditOptions::new();
    opts.base = Some(s.read(c)?.version);
    let r = s.edit_ops(
        c,
        vec![
            Edit::List {
                root: SHAPES.into(),
                ops: vec![ListOp::Insert {
                    key: k(key),
                    at: Anchor::End,
                    value: Value::Null,
                }],
            },
            Edit::Map {
                root: FIELDS.into(),
                ops: vec![MapOp::Set {
                    key: format!("{key}:title"),
                    value: Value::str(title),
                }],
            },
        ],
        &opts,
    )?;
    Ok(r.version)
}

fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {}", what()))
    }
}

// ---------- the size limit ----------

/// Six elements and fields: three shapes, one field each (the cap in these rows).
const AT_CAP: &str = "s1\ttitle=A\ns2\ttitle=B\ns3\ttitle=C\n";
/// Eight: one shape and one field over.
const OVER_CAP: &str = "s1\ttitle=A\ns2\ttitle=B\ns3\ttitle=C\ns4\ttitle=D\n";

fn capped(tag: &str) -> Fx {
    fx_with(tag, |c| c.keyed_max_entries = 6)
}

/// A file over the cap isn't live: declined `live_too_large` (`MPC-E415`, exit 4), nothing
/// stored; and the host's fallback opens it at tier 0, byte for byte.
fn row_keyed_over_the_cap_is_declined_and_opens_at_tier_0() -> Result<(), String> {
    const ROW: &str = "row_keyed_over_the_cap_is_declined_and_opens_at_tier_0";
    let f = capped("over-cap");
    f.write(OVER_CAP);
    let e = match f.try_open() {
        Ok(_) => return Err(format!("{ROW}: a file over the cap opened live")),
        Err(e) => e,
    };
    let SessionError::Declined(d) = &e else {
        return Err(format!("{ROW}: expected a decline, got {e:?}"));
    };
    need(d.code == "live_too_large", ROW, || {
        format!("the decline's code: {}", d.code)
    })?;
    let rep = e.report();
    need(rep.id() == "MPC-E415", ROW, || {
        format!("the registry id: {}", rep.id())
    })?;
    need(rep.exit() == 4, ROW, || format!("the exit: {}", rep.exit()))?;
    need(f.store.find(&key()).unwrap().is_none(), ROW, || {
        "the decline stored no live doc".into()
    })?;
    // the host's fallback: the file at tier 0
    let b = f
        .mgr
        .open_blob_file(Arc::new(AnyFile), &key(), &f.path(), &f.ws)
        .map_err(|e| format!("{ROW}: the tier-0 open failed: {e}"))?;
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("ana".into(), Rights::full());
    let c = b
        .join(
            Participant::new("ana", "ana", "#123456", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let mut held = Vec::new();
    b.read(c).unwrap().reader.read_to_end(&mut held).unwrap();
    need(held == OVER_CAP.as_bytes(), ROW, || {
        "the tier-0 doc holds the file".into()
    })?;
    need(f.bytes() == OVER_CAP, ROW, || {
        "the file is untouched".into()
    })
}

#[test]
fn keyed_over_the_cap_is_declined_and_opens_at_tier_0() {
    row_keyed_over_the_cap_is_declined_and_opens_at_tier_0().unwrap();
}

/// A file exactly at the cap opens live, and holds the file.
#[test]
fn keyed_at_the_cap_opens_live() {
    let f = capped("at-cap");
    f.write(AT_CAP);
    let s = f.open();
    let p = f.person(&s);
    assert_eq!(text(&s, p), AT_CAP);
    // a change that doesn't grow it passes at the cap
    set_field(&s, p, "s2", "title", "B2").unwrap();
    assert_eq!(text(&s, p), "s1\ttitle=A\ns2\ttitle=B2\ns3\ttitle=C\n");
}

/// At the cap, a change that would grow the doc is refused (`AtLimit`, `elements and
/// fields`, exit 2) with nothing applied; one that doesn't grow it still passes.
fn row_keyed_growth_past_the_cap_is_refused() -> Result<(), String> {
    const ROW: &str = "row_keyed_growth_past_the_cap_is_refused";
    let f = capped("grow");
    f.write(AT_CAP);
    let s = f.open();
    let p = f.person(&s);
    let v = s.status().unwrap().version;
    let e = match add_shape(&s, p, "s4", "D") {
        Ok(v) => return Err(format!("{ROW}: the doc grew past the cap (version {v})")),
        Err(e) => e,
    };
    need(
        keyed_refusal_from(&e) == Some((6, 8, LimitSource::HostWide)),
        ROW,
        || format!("expected KeyedAtLimit at 6 (host-wide) to 8, got {e:?}"),
    )?;
    need(e.report().id() == "MPC-E822", ROW, || {
        format!("the registry id: {}", e.report().id())
    })?;
    need(s.status().unwrap().version == v, ROW, || {
        "nothing was applied".into()
    })?;
    need(text(&s, p) == AT_CAP, ROW, || "the doc is as it was".into())?;
    // a set doesn't grow it
    set_field(&s, p, "s1", "title", "A2").map_err(|e| format!("{ROW}: a set: {e}"))?;
    Ok(())
}

#[test]
fn keyed_growth_past_the_cap_is_refused() {
    row_keyed_growth_past_the_cap_is_refused().unwrap();
}

/// At the cap, an outside save that grows the doc declines the file (`live_too_large`); the
/// doc keeps what it had and the file is never written over.
fn row_keyed_outside_save_past_the_cap_declines_the_file() -> Result<(), String> {
    const ROW: &str = "row_keyed_outside_save_past_the_cap_declines_the_file";
    let f = capped("save-over");
    f.write(AT_CAP);
    let s = f.open();
    let p = f.person(&s);
    f.advance(1_000);
    app_save(&f.path(), OVER_CAP);
    let out = s.file_changed().unwrap();
    let DiskCheck::Declined(d) = &out else {
        return Err(format!("{ROW}: expected the file declined, got {out:?}"));
    };
    need(d.code == "live_too_large", ROW, || {
        format!("the decline's code: {}", d.code)
    })?;
    need(text(&s, p) == AT_CAP, ROW, || {
        "the doc keeps what it had".into()
    })?;
    need(f.bytes() == OVER_CAP, ROW, || {
        "the file isn't written over".into()
    })?;
    need(
        f.events().iter().any(|e| {
            matches!(
                e,
                AuditEvent::FileDeclined { code, .. } if *code == "live_too_large"
            )
        }),
        ROW,
        || "a FileDeclined event names the code".into(),
    )
}

#[test]
fn keyed_outside_save_past_the_cap_declines_the_file() {
    row_keyed_outside_save_past_the_cap_declines_the_file().unwrap();
}

/// The planted fault "keyed limit not applied": each limit row goes red, by name.
#[test]
fn the_planted_keyed_limit_not_applied_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::LimitNotApplied, || {
        for (name, row) in [
            (
                "row_keyed_over_the_cap_is_declined_and_opens_at_tier_0",
                row_keyed_over_the_cap_is_declined_and_opens_at_tier_0
                    as fn() -> Result<(), String>,
            ),
            (
                "row_keyed_growth_past_the_cap_is_refused",
                row_keyed_growth_past_the_cap_is_refused,
            ),
            (
                "row_keyed_outside_save_past_the_cap_declines_the_file",
                row_keyed_outside_save_past_the_cap_declines_the_file,
            ),
        ] {
            let e = row().expect_err(&format!("planted, {name} must fail"));
            assert!(e.starts_with(&format!("{name}: ")), "{e}");
        }
    });
}

/// The default cap is the CSV grid's measured one until a deck-shaped measurement sets it.
#[test]
fn the_default_keyed_cap_is_the_grids() {
    let c = Config::default();
    assert_eq!(c.keyed_max_entries, 25_000);
    assert_eq!(c.keyed_max_entries, c.grid_max_cells);
    assert!(c
        .limits()
        .iter()
        .any(|(n, v)| *n == "keyed_max_entries" && *v == 25_000));
}

// ---------- a limit of any size ----------

/// Insert a shape `key` with no field, at the end, as `c`: one element.
fn add_bare_shape(s: &Session, c: ConnId, key: &str) -> Result<u64, SessionError> {
    let mut opts = EditOptions::new();
    opts.base = Some(s.read(c)?.version);
    let r = s.edit_ops(
        c,
        vec![Edit::List {
            root: SHAPES.into(),
            ops: vec![ListOp::Insert {
                key: k(key),
                at: Anchor::End,
                value: Value::Null,
            }],
        }],
        &opts,
    )?;
    Ok(r.version)
}

/// The size refusal's limit and what the change would have made it, if `e` is one.
fn keyed_refusal(e: &SessionError) -> Option<(usize, usize)> {
    keyed_refusal_from(e).map(|(l, w, _)| (l, w))
}

/// The size refusal's limit, what the change would have made it, and where the limit came
/// from, if `e` is one (`KeyedAtLimit`; never the grid's `AtLimit`).
fn keyed_refusal_from(e: &SessionError) -> Option<(usize, usize, LimitSource)> {
    match e {
        SessionError::KeyedAtLimit {
            limit,
            would_be,
            from,
            ..
        } => Some((*limit, *would_be, *from)),
        _ => None,
    }
}

/// A limit is compared exactly whatever its size: one past `i64::MAX`, up to `usize::MAX`,
/// lets the doc grow (before, a limit that didn't fit an `i64` wrapped negative and refused
/// every growing change).
#[test]
fn keyed_limits_of_any_size_let_the_doc_grow() {
    let big = usize::try_from(i64::MAX).expect("a 64-bit host");
    for limit in [big, big + 1, usize::MAX - 1, usize::MAX] {
        let f = fx_with("anylimit", |c| c.keyed_max_entries = limit);
        f.write(AT_CAP);
        let s = f.open();
        let p = f.person(&s);
        let grew = add_shape(&s, p, "s4", "D");
        assert!(
            grew.is_ok(),
            "keyed limit {limit}: the doc didn't grow: {grew:?}"
        );
        assert_eq!(text(&s, p), OVER_CAP, "keyed limit {limit}");
    }
}

/// A limit of 0: an empty file opens live, and any growth is refused; a file with one
/// element is over it (`live_too_large`).
#[test]
fn keyed_limit_zero() {
    let f = fx_with("limit0", |c| c.keyed_max_entries = 0);
    f.write("");
    let s = f.open();
    let p = f.person(&s);
    let e = add_bare_shape(&s, p, "s1").expect_err("limit 0: growth is refused");
    assert_eq!(keyed_refusal(&e), Some((0, 1)), "limit 0: {e:?}");
    assert_eq!(e.report().id(), "MPC-E822");
    assert_eq!(text(&s, p), "");
    let f = fx_with("limit0-file", |c| c.keyed_max_entries = 0);
    f.write("s1\n");
    let e = f.try_open().err().expect("limit 0: one element is over it");
    assert!(
        matches!(&e, SessionError::Declined(d) if d.code == "live_too_large"),
        "limit 0: {e:?}"
    );
}

/// A limit of 1: one element fits, the second is refused.
#[test]
fn keyed_limit_one() {
    let f = fx_with("limit1", |c| c.keyed_max_entries = 1);
    f.write("");
    let s = f.open();
    let p = f.person(&s);
    add_bare_shape(&s, p, "s1").expect("limit 1: the first element fits");
    let e = add_bare_shape(&s, p, "s2").expect_err("limit 1: the second is refused");
    assert_eq!(keyed_refusal(&e), Some((1, 2)), "limit 1: {e:?}");
    assert_eq!(text(&s, p), "s1\n");
    // a file at it opens live
    let f = fx_with("limit1-file", |c| c.keyed_max_entries = 1);
    f.write("s1\n");
    let s = f.open();
    let p = f.person(&s);
    assert_eq!(text(&s, p), "s1\n");
}

/// At the default limit (25,000) the refusal (`MPC-E822`) still fires.
#[test]
fn keyed_default_limit_still_refuses_at_it() {
    let f = fx("default-limit");
    let limit = Config::default().keyed_max_entries;
    // limit / 2 shapes with one field each: exactly at the limit
    let file: String = (0..limit / 2).map(|i| format!("s{i}\ttitle=t\n")).collect();
    f.write(&file);
    let s = f.open();
    let p = f.person(&s);
    let e = add_shape(&s, p, "extra", "E").expect_err("growth past the default is refused");
    assert_eq!(keyed_refusal(&e), Some((limit, limit + 2)), "{e:?}");
    assert_eq!(e.report().id(), "MPC-E822");
    assert_eq!(
        keyed_refusal_from(&e).map(|r| r.2),
        Some(LimitSource::HostWide)
    );
    // a change that doesn't grow it passes
    set_field(&s, p, "s0", "title", "u").expect("a set at the limit passes");
}

// ---------- the limit per file type ----------

/// `Deck` with a recommended limit of its own ([`Pack::keyed_entries_default`]).
struct DeckWithDefault(usize);

impl Pack for DeckWithDefault {
    fn schema(&self) -> Schema {
        Deck.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        Deck.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        Deck.encode(c, codec)
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], x: &[u8]) -> Result<Reconciled, Declined> {
        Deck.reconcile(synced, bytes, x)
    }
    fn keyed_entries_default(&self) -> Option<usize> {
        Some(self.0)
    }
}

impl Fx {
    fn try_open_pack(&self, pack: Arc<dyn Pack>) -> Result<Arc<Session>, SessionError> {
        self.mgr.open_file(pack, &key(), &self.path(), &self.ws)
    }
}

/// A report's `limit` and `limit_source` details.
fn limit_details(rep: &ErrorReport) -> (Option<u64>, Option<String>) {
    (
        rep.detail("limit").and_then(ErrorValue::as_u64),
        rep.detail("limit_source")
            .and_then(ErrorValue::as_str)
            .map(str::to_owned),
    )
}

/// The rows of one limit's source: under the host's settings `set` and `pack`, the limit is 6
/// (the cap of [`AT_CAP`]) and comes from `source`. A file over it is declined
/// `live_too_large` (`MPC-E415`), a change past it is refused (`MPC-E822`), and an outside save
/// past it declines the file, each naming the limit and its source in its details.
fn row_limit_from(
    row: &str,
    set: impl Fn(&mut Config),
    pack: impl Fn() -> Arc<dyn Pack>,
    source: LimitSource,
) -> Result<(), String> {
    let want = (Some(6), Some(source.as_str().to_owned()));
    // the config resolves it
    let f = fx_with(row, &set);
    let got = f.mgr.config().keyed_limit(&*pack());
    need((got.limit, got.source) == (6, source), row, || {
        format!("Config::keyed_limit: {got:?}")
    })?;
    // a file over it is declined, naming it
    f.write(OVER_CAP);
    let e = match f.try_open_pack(pack()) {
        Ok(_) => return Err(format!("{row}: a file over the limit opened live")),
        Err(e) => e,
    };
    let rep = e.report();
    need(rep.id() == "MPC-E415", row, || format!("the open: {e:?}"))?;
    need(limit_details(&rep) == want, row, || {
        format!("the decline's details: {:?}", limit_details(&rep))
    })?;
    // at it, a change that grows the doc is refused, naming it
    let f = fx_with(row, &set);
    f.write(AT_CAP);
    let s = f
        .try_open_pack(pack())
        .map_err(|e| format!("{row}: a file at the limit didn't open: {e}"))?;
    let p = f.person(&s);
    let e = match add_shape(&s, p, "s4", "D") {
        Ok(v) => return Err(format!("{row}: the doc grew past the limit (version {v})")),
        Err(e) => e,
    };
    need(keyed_refusal_from(&e) == Some((6, 8, source)), row, || {
        format!("the refusal: {e:?}")
    })?;
    let rep = e.report();
    need(rep.id() == "MPC-E822", row, || {
        format!("the refusal's id: {}", rep.id())
    })?;
    need(limit_details(&rep) == want, row, || {
        format!("the refusal's details: {:?}", limit_details(&rep))
    })?;
    // an outside save past it declines the file, naming it
    f.advance(1_000);
    app_save(&f.path(), OVER_CAP);
    let out = s.file_changed().map_err(|e| format!("{row}: {e}"))?;
    let DiskCheck::Declined(d) = &out else {
        return Err(format!("{row}: expected the file declined, got {out:?}"));
    };
    let rep = Refusal::report(d);
    need(rep.id() == "MPC-E415", row, || format!("the save: {d:?}"))?;
    need(limit_details(&rep) == want, row, || {
        format!("the save's details: {:?}", limit_details(&rep))
    })
}

/// The host's limit for the file type wins over the pack's default and the host-wide one; a
/// limit set for another type doesn't apply.
#[test]
fn keyed_limit_from_the_hosts_per_pack_setting() {
    row_limit_from(
        "per-pack",
        |c| {
            c.keyed_max_entries = 100;
            c.keyed_max_entries_by_pack.insert("deck-lite".into(), 6);
            c.keyed_max_entries_by_pack.insert("another-type".into(), 1);
        },
        || Arc::new(DeckWithDefault(50)),
        LimitSource::HostPerPack,
    )
    .unwrap();
}

/// With no limit of the host's for the type, the pack's own default applies, in place of the
/// host-wide one.
#[test]
fn keyed_limit_from_the_packs_default() {
    row_limit_from(
        "pack-default",
        |c| {
            c.keyed_max_entries = 100;
            c.keyed_max_entries_by_pack.insert("another-type".into(), 1);
        },
        || Arc::new(DeckWithDefault(6)),
        LimitSource::PackDefault,
    )
    .unwrap();
}

/// With neither, the host-wide limit applies.
#[test]
fn keyed_limit_from_the_host_wide_setting() {
    row_limit_from(
        "host-wide",
        |c| {
            c.keyed_max_entries = 6;
            c.keyed_max_entries_by_pack.insert("another-type".into(), 1);
        },
        || Arc::new(Deck),
        LimitSource::HostWide,
    )
    .unwrap();
}

/// The host owns the limit: a pack's default above the host-wide limit is capped at it (the
/// source is the host-wide limit). A type that needs more gets it only from the host's limit
/// for that type.
#[test]
fn a_packs_default_is_capped_at_the_host_wide_limit() {
    let f = fx_with("capped", |c| c.keyed_max_entries = 6);
    f.write(OVER_CAP);
    let e = f
        .try_open_pack(Arc::new(DeckWithDefault(10)))
        .err()
        .expect("capped at six: a file of eight is over it");
    let rep = e.report();
    assert_eq!(rep.id(), "MPC-E415", "{e:?}");
    assert_eq!(
        limit_details(&rep),
        (Some(6), Some("host_wide".to_owned())),
        "{e:?}"
    );
    let f = fx_with("capped-grow", |c| c.keyed_max_entries = 6);
    f.write(AT_CAP);
    let s = f
        .try_open_pack(Arc::new(DeckWithDefault(10)))
        .expect("six is at the cap");
    let p = f.person(&s);
    let e = add_bare_shape(&s, p, "s4").expect_err("seven is past the host-wide six");
    assert_eq!(
        keyed_refusal_from(&e),
        Some((6, 7, LimitSource::HostWide)),
        "{e:?}"
    );
    // the host's limit for the type raises it
    let f = fx_with("capped-raised", |c| {
        c.keyed_max_entries = 6;
        c.keyed_max_entries_by_pack.insert("deck-lite".into(), 10);
    });
    f.write(OVER_CAP);
    let s = f
        .try_open_pack(Arc::new(DeckWithDefault(7)))
        .expect("the host's ten for the type wins");
    let p = f.person(&s);
    add_bare_shape(&s, p, "s5").expect("nine fits");
    add_bare_shape(&s, p, "s6").expect("ten fits");
    let e = add_bare_shape(&s, p, "s7").expect_err("eleven doesn't");
    assert_eq!(
        keyed_refusal_from(&e),
        Some((10, 11, LimitSource::HostPerPack)),
        "{e:?}"
    );
}

/// A pack's default below the host-wide limit lowers it for that type (source `pack_default`).
#[test]
fn a_packs_default_below_the_host_wide_limit_lowers_it() {
    let f = fx_with("lowered", |c| c.keyed_max_entries = 100);
    f.write(AT_CAP);
    let s = f
        .try_open_pack(Arc::new(DeckWithDefault(7)))
        .expect("six is under the pack's seven");
    let p = f.person(&s);
    add_bare_shape(&s, p, "s4").expect("seven fits");
    let e = add_bare_shape(&s, p, "s5").expect_err("eight is past the pack's seven");
    assert_eq!(
        keyed_refusal_from(&e),
        Some((7, 8, LimitSource::PackDefault)),
        "{e:?}"
    );
    let rep = e.report();
    assert_eq!(
        limit_details(&rep),
        (Some(7), Some("pack_default".to_owned()))
    );
}

/// `Config::keyed_limit`'s order: the host's for the type, whatever it is; else the pack's
/// default when it is below the host-wide limit; else the host-wide limit.
#[test]
fn config_keyed_limit_takes_the_first_set() {
    let mut c = Config::default();
    let got = |c: &Config, p: &dyn Pack| {
        let l = c.keyed_limit(p);
        (l.limit, l.source)
    };
    assert_eq!(got(&c, &Deck), (25_000, LimitSource::HostWide));
    assert_eq!(got(&c, &DeckWithDefault(7)), (7, LimitSource::PackDefault));
    // a pack's default never raises it: capped (equal is the host's own)
    let host_wide = (25_000, LimitSource::HostWide);
    assert_eq!(got(&c, &DeckWithDefault(25_000)), host_wide);
    assert_eq!(got(&c, &DeckWithDefault(25_001)), host_wide);
    assert_eq!(got(&c, &DeckWithDefault(usize::MAX)), host_wide);
    assert_eq!(
        got(&c, &DeckWithDefault(24_999)),
        (24_999, LimitSource::PackDefault)
    );
    c.keyed_max_entries_by_pack.insert("another-type".into(), 3);
    assert_eq!(got(&c, &Deck), (25_000, LimitSource::HostWide));
    assert_eq!(got(&c, &DeckWithDefault(7)), (7, LimitSource::PackDefault));
    c.keyed_max_entries_by_pack.insert("deck-lite".into(), 0);
    assert_eq!(got(&c, &Deck), (0, LimitSource::HostPerPack));
    assert_eq!(got(&c, &DeckWithDefault(7)), (0, LimitSource::HostPerPack));
    c.keyed_max_entries_by_pack
        .insert("deck-lite".into(), usize::MAX);
    assert_eq!(
        got(&c, &DeckWithDefault(7)),
        (usize::MAX, LimitSource::HostPerPack)
    );
    // the names a report's details give
    let names: Vec<&str> = LimitSource::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(names, ["host_per_pack", "pack_default", "host_wide"]);
}

// ---------- the outside-save rule (the review's B2) ----------

/// Every `BlobConflict` event: (current, kept, copy, lease holder).
type ConflictEvent = (u64, u64, Option<String>, Option<String>);
fn conflict_events(f: &Fx) -> Vec<ConflictEvent> {
    f.events()
        .into_iter()
        .filter_map(|e| match e {
            AuditEvent::BlobConflict {
                current,
                kept,
                copy,
                lease_holder,
                ..
            } => Some((current, kept, copy, lease_holder)),
            _ => None,
        })
        .collect()
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

const ONE: &str = "s1\ttitle=A\ns2\ttitle=B\n";
/// The agent's write: s1's title.
const AGENTS: &str = "s1\ttitle=Agent\ns2\ttitle=B\n";
/// The app's save from the buffer it opened before the agent's write: s2's title changed.
const PERSONS: &str = "s1\ttitle=A\ns2\ttitle=Person\n";

/// The agent writes s1's title through the live doc and the kit writes the file.
fn agent_write(f: &Fx, s: &Session, bot: ConnId) -> u64 {
    let v = set_field(s, bot, "s1", "title", "Agent").unwrap();
    assert!(
        s.flush(bot).unwrap().written,
        "control: the kit wrote the file"
    );
    assert_eq!(
        f.bytes(),
        AGENTS,
        "control: the file holds the agent's write"
    );
    v
}

/// Checks the kept-both outcome of an app's stale save `out`: the save is the live doc and on
/// disk; the kit's version is the version `kept` and the conflict copy, byte for byte; one
/// `BlobConflict` names the copy.
fn kept_both(f: &Fx, s: &Session, bot: ConnId, out: DiskCheck, row: &str) -> Result<(), String> {
    let DiskCheck::Conflict {
        version,
        kept,
        copy,
    } = out
    else {
        return Err(format!("{row}: expected a conflict, got {out:?}"));
    };
    let copy = copy.ok_or_else(|| format!("{row}: a conflict copy was written"))?;
    let name = copy.file_name().unwrap().to_str().unwrap().to_owned();
    need(
        name.starts_with("talk (conflict bot 2026-10-27 ") && name.ends_with(").deck"),
        row,
        || format!("the copy has the kit's name, after the agent: {name}"),
    )?;
    need(copy.parent() == Some(f.ws.as_path()), row, || {
        format!("the copy is beside the file: {}", copy.display())
    })?;
    let held = fs::read_to_string(&copy).unwrap();
    need(held == AGENTS, row, || {
        format!("the copy holds the kit's version: {held:?}")
    })?;
    let kept_bytes = s.file_at(bot, kept).map(|b| b.bytes);
    need(kept_bytes.as_deref() == Ok(AGENTS.as_bytes()), row, || {
        format!("the kept version holds the kit's version: {kept_bytes:?}")
    })?;
    need(s.status().unwrap().version == version, row, || {
        "the save's change is current".into()
    })?;
    need(text(s, bot) == PERSONS, row, || {
        format!("the save is the live doc: {:?}", text(s, bot))
    })?;
    need(f.bytes() == PERSONS, row, || {
        "the save stays on disk".into()
    })?;
    let ev = conflict_events(f);
    need(ev.len() == 1, row, || {
        format!("one BlobConflict for the copy: {ev:?}")
    })?;
    need(
        ev[0].1 == kept && ev[0].2.as_deref() == Some(&*name) && ev[0].3.is_none(),
        row,
        || format!("the event names the kept version and the copy: {ev:?}"),
    )?;
    need(
        f.events()
            .iter()
            .any(|e| matches!(e, AuditEvent::ExternalEdit { seq, .. } if *seq == version)),
        row,
        || "the save is an outside edit on the audit record".into(),
    )
}

/// After a kit write (an agent's change, written back), an app that opened the file before it
/// saves: both are kept. Red before this rule: the save was applied straight onto the live doc
/// (the agent's title reverted), with no copy, no version of it on purpose and no event.
fn row_keyed_stale_save_after_a_kit_write_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_keyed_stale_save_after_a_kit_write_keeps_both";
    let f = fx("stale");
    f.write(ONE);
    let s = f.open();
    let bot = f.agent(&s);
    agent_write(&f, &s, bot);
    f.advance(1_000);
    app_save(&f.path(), PERSONS);
    let out = s.file_changed().unwrap();
    kept_both(&f, &s, bot, out, ROW)?;
    // the save that kept both is the last outside save: the app's next save is plain
    f.advance(1_000);
    app_save(&f.path(), "s1\ttitle=A\ns2\ttitle=Person 2\n");
    let out = s.file_changed().unwrap();
    need(matches!(out, DiskCheck::Ingested { .. }), ROW, || {
        format!("the app's next save is a plain outside save: {out:?}")
    })?;
    need(conflict_events(&f).len() == 1, ROW, || {
        "the conflict event is emitted once".into()
    })?;
    need(names_in(&f.ws).len() == 2, ROW, || {
        format!("one copy beside the file: {:?}", names_in(&f.ws))
    })
}

#[test]
fn keyed_stale_save_after_a_kit_write_keeps_both() {
    row_keyed_stale_save_after_a_kit_write_keeps_both().unwrap();
}

/// The same across a restart between the kit's write and the app's save: decided from the
/// store (the change the file was last synced at), not from memory.
fn row_keyed_stale_save_after_a_kit_write_across_a_restart() -> Result<(), String> {
    const ROW: &str = "row_keyed_stale_save_after_a_kit_write_across_a_restart";
    let mut f = fx("stale-restart");
    f.write(ONE);
    let s = f.open();
    let bot = f.agent(&s);
    agent_write(&f, &s, bot);
    drop(s);
    f.restart();
    let s = f.open();
    let bot = f.agent(&s);
    f.advance(1_000);
    app_save(&f.path(), PERSONS);
    let out = s.file_changed().unwrap();
    kept_both(&f, &s, bot, out, ROW)
}

#[test]
fn keyed_stale_save_after_a_kit_write_across_a_restart() {
    row_keyed_stale_save_after_a_kit_write_across_a_restart().unwrap();
}

/// A save that lands on unwritten changes keeps both too (one rule, never a merge by key):
/// the agent's change isn't written yet when the app saves (s2's title). The save becomes the
/// live doc exactly; the kit's version (with the agent's change) is a version and a conflict
/// copy; one conflict event; the doc then holds the file, so nothing is written. Red before the
/// rule: the save was merged by key onto the agent's change, with no copy and no event.
fn row_keyed_save_on_unwritten_changes_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_keyed_save_on_unwritten_changes_keeps_both";
    let f = fx("dirty");
    f.write(ONE);
    let s = f.open();
    let bot = f.agent(&s);
    set_field(&s, bot, "s1", "title", "Agent").unwrap();
    f.advance(1_000);
    const SAVE: &str = "s1\ttitle=A\ns2\ttitle=First\n";
    app_save(&f.path(), SAVE);
    let out = s.file_changed().unwrap();
    let DiskCheck::Conflict {
        kept,
        copy: Some(copy),
        ..
    } = out
    else {
        return Err(format!(
            "{ROW}: a save on unwritten changes keeps both: {out:?}"
        ));
    };
    need(text(&s, bot) == SAVE, ROW, || {
        format!("the save is the live doc exactly: {:?}", text(&s, bot))
    })?;
    let held = fs::read_to_string(&copy).unwrap();
    need(held == AGENTS, ROW, || {
        format!("the copy holds the kit's version: {held:?}")
    })?;
    let kept_bytes = s.file_at(bot, kept).map(|b| b.bytes);
    need(kept_bytes.as_deref() == Ok(AGENTS.as_bytes()), ROW, || {
        format!("the kept version holds the kit's version: {kept_bytes:?}")
    })?;
    need(conflict_events(&f).len() == 1, ROW, || {
        format!("one conflict event: {:?}", conflict_events(&f))
    })?;
    need(!s.flush(bot).unwrap().written, ROW, || {
        "the doc holds the file: nothing is written".into()
    })?;
    need(f.bytes() == SAVE, ROW, || "the save stays on disk".into())
}

#[test]
fn keyed_save_on_unwritten_changes_keeps_both() {
    row_keyed_save_on_unwritten_changes_keeps_both().unwrap();
}

/// The same with a restart between the agent's change and the app's save (the process
/// stopped before the write-back: the change is in the store, never in the file).
fn row_keyed_save_on_unwritten_changes_keeps_both_after_a_restart() -> Result<(), String> {
    const ROW: &str = "row_keyed_save_on_unwritten_changes_keeps_both_after_a_restart";
    let mut f = fx("dirty-restart");
    f.write(ONE);
    let s = f.open();
    let bot = f.agent(&s);
    set_field(&s, bot, "s1", "title", "Agent").unwrap();
    drop(s);
    f.restart();
    let s = f.open();
    let bot = f.agent(&s);
    f.advance(1_000);
    app_save(&f.path(), PERSONS);
    let out = s.file_changed().unwrap();
    need(matches!(out, DiskCheck::Conflict { .. }), ROW, || {
        format!("a save on unwritten changes keeps both after a restart: {out:?}")
    })?;
    need(text(&s, bot) == PERSONS, ROW, || {
        format!("the save is the live doc: {:?}", text(&s, bot))
    })?;
    let ev = conflict_events(&f);
    need(ev.len() == 1, ROW, || format!("one conflict event: {ev:?}"))?;
    let kept = s.file_at(bot, ev[0].1).map(|b| b.bytes);
    need(kept.as_deref() == Ok(AGENTS.as_bytes()), ROW, || {
        format!("the kept version holds the agent's change: {kept:?}")
    })
}

#[test]
fn keyed_save_on_unwritten_changes_keeps_both_after_a_restart() {
    row_keyed_save_on_unwritten_changes_keeps_both_after_a_restart().unwrap();
}

/// The planted fault "dirty save merged by key": both rows go red, by name.
#[test]
fn the_planted_dirty_save_merged_by_key_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::DirtySaveMergedByKey, || {
        for (name, row) in [
            (
                "row_keyed_save_on_unwritten_changes_keeps_both",
                row_keyed_save_on_unwritten_changes_keeps_both as fn() -> Result<(), String>,
            ),
            (
                "row_keyed_save_on_unwritten_changes_keeps_both_after_a_restart",
                row_keyed_save_on_unwritten_changes_keeps_both_after_a_restart,
            ),
        ] {
            let e = row().expect_err(&format!("planted, {name} must fail"));
            assert!(e.starts_with(&format!("{name}: ")), "{e}");
            assert!(e.contains("keeps both"), "{e}");
        }
    });
}

/// Unchanged: with no kit write since the last outside save (the file as imported, then an
/// outside save), each save applies plainly, before and after a restart: no copy, no event.
#[test]
fn keyed_outside_save_with_no_kit_write_since_applies_plainly() {
    let mut f = fx("plain");
    f.write(ONE);
    let s = f.open();
    let p = f.person(&s);
    for (i, save) in ["s1\ttitle=X\ns2\ttitle=B\n", "s1\ttitle=Y\ns2\ttitle=B\n"]
        .into_iter()
        .enumerate()
    {
        f.advance(1_000);
        app_save(&f.path(), save);
        let out = s.file_changed().unwrap();
        assert!(
            matches!(out, DiskCheck::Ingested { .. }),
            "save {i}: {out:?}"
        );
        assert_eq!(text(&s, p), save, "save {i}");
    }
    drop(s);
    f.restart();
    let s = f.open();
    let p = f.person(&s);
    f.advance(1_000);
    app_save(&f.path(), "s1\ttitle=Z\ns2\ttitle=B\n");
    let out = s.file_changed().unwrap();
    assert!(
        matches!(out, DiskCheck::Ingested { .. }),
        "after a restart: {out:?}"
    );
    assert_eq!(text(&s, p), "s1\ttitle=Z\ns2\ttitle=B\n");
    assert_eq!(names_in(&f.ws), [NAME], "no conflict copy");
    assert!(conflict_events(&f).is_empty(), "no conflict event");
}

/// Unchanged: a save of exactly what the kit wrote records nothing, before and after a
/// restart.
#[test]
fn keyed_save_identical_to_a_kit_write_is_a_no_op() {
    let mut f = fx("same");
    f.write(ONE);
    let s = f.open();
    let bot = f.agent(&s);
    let v = agent_write(&f, &s, bot);
    let versions = s.versions(bot).unwrap().len();
    f.advance(1_000);
    app_save(&f.path(), AGENTS);
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Unchanged);
    assert_eq!(s.status().unwrap().version, v);
    assert_eq!(s.versions(bot).unwrap().len(), versions);
    drop(s);
    f.restart();
    let s = f.open();
    f.advance(1_000);
    app_save(&f.path(), AGENTS);
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Unchanged);
    assert_eq!(s.status().unwrap().version, v);
    assert_eq!(names_in(&f.ws), [NAME], "no conflict copy");
    assert!(conflict_events(&f).is_empty(), "no conflict event");
}

/// `Host::conflict_copy_name` names the copy first (as on tier 0).
#[test]
fn keyed_conflict_copy_takes_the_hosts_name() {
    fn persons(file_name: &str, _at_ms: u64, who: &str) -> Option<String> {
        let (stem, ext) = file_name.rsplit_once('.').unwrap();
        Some(format!("{stem} ({who}'s version).{ext}"))
    }
    let f = fx("host-name");
    *f.host.copy_names.lock().unwrap() = Some(persons as CopyNamer);
    f.write(ONE);
    let s = f.open();
    let bot = f.agent(&s);
    agent_write(&f, &s, bot);
    f.advance(1_000);
    app_save(&f.path(), PERSONS);
    let out = s.file_changed().unwrap();
    let DiskCheck::Conflict { copy: Some(c), .. } = out else {
        panic!("expected a conflict with a copy, got {out:?}");
    };
    assert_eq!(c.file_name().unwrap(), "talk (bot's version).deck");
    assert_eq!(fs::read_to_string(&c).unwrap(), AGENTS);
}

/// The kit's conflict copies are removed on `tick` after `Config::conflict_copy_keep_ms`, as
/// tier 0's are, also after a restart (the copies are recorded in the store): not
/// before the keep, and never a copy a person edited. Red before the cleanup: the copy stayed.
fn row_keyed_conflict_copy_is_removed_on_tick() -> Result<(), String> {
    const ROW: &str = "row_keyed_conflict_copy_is_removed_on_tick";
    const KEEP: u64 = 600_000;
    let mut f = fx_with("copy-tick", |c| c.conflict_copy_keep_ms = KEEP);
    f.write(ONE);
    let s = f.open();
    let bot = f.agent(&s);
    agent_write(&f, &s, bot);
    f.advance(1_000);
    app_save(&f.path(), PERSONS);
    let DiskCheck::Conflict { copy: Some(c1), .. } = s.file_changed().unwrap() else {
        return Err(format!("{ROW}: control: the first save keeps both"));
    };
    // a second copy, which a person then edits
    set_field(&s, bot, "s1", "title", "Agent 2").unwrap();
    assert!(
        s.flush(bot).unwrap().written,
        "control: the kit's second write"
    );
    f.advance(1_000);
    app_save(&f.path(), "s1\ttitle=A\ns2\ttitle=Person 2\n");
    let DiskCheck::Conflict { copy: Some(c2), .. } = s.file_changed().unwrap() else {
        return Err(format!("{ROW}: control: the second save keeps both"));
    };
    fs::write(&c2, "s1\ttitle=edited by a person\n").unwrap();
    // inside the keep: both stay
    let rep = f.mgr.tick().unwrap();
    need(rep.errors.is_empty(), ROW, || {
        format!("tick errors: {:?}", rep.errors)
    })?;
    need(c1.exists() && c2.exists(), ROW, || {
        "both copies stay inside the keep".into()
    })?;
    // a restart: the records are the store's
    drop(s);
    f.restart();
    let s = f.open();
    let _held = f.agent(&s);
    f.advance(KEEP + 60_000);
    let rep = f.mgr.tick().unwrap();
    need(rep.errors.is_empty(), ROW, || {
        format!("tick errors: {:?}", rep.errors)
    })?;
    need(!c1.exists(), ROW, || {
        format!("the copy is removed after its keep: {}", c1.display())
    })?;
    need(
        rep.conflict_copies_removed
            .iter()
            .any(|(k, p)| k.path == NAME && *p == c1),
        ROW,
        || format!("the tick reports it: {:?}", rep.conflict_copies_removed),
    )?;
    need(c2.exists(), ROW, || "a copy a person edited stays".into())?;
    need(
        f.bytes() == "s1\ttitle=A\ns2\ttitle=Person 2\n",
        ROW,
        || "the file is untouched".into(),
    )
}

#[test]
fn keyed_conflict_copy_is_removed_on_tick() {
    row_keyed_conflict_copy_is_removed_on_tick().unwrap();
}

/// A purge of a live doc reaches its conflict copies, as tier 0's purge does: a copy still
/// holding its kept version's content, which the purge erased, is deleted with its record
/// and counted; a copy a person edited may hold erased content the kit can't vouch for, so it
/// is left and reported (`NotReached::ConflictCopies`). Red before: the purge left the copy
/// on disk and in `live_copies`, unreported.
fn row_keyed_purge_removes_live_conflict_copies() -> Result<(), String> {
    const ROW: &str = "row_keyed_purge_removes_live_conflict_copies";
    let f = fx("copy-purge");
    f.write(ONE);
    let s = f.open();
    let bot = f.agent(&s);
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("own".into(), Rights::full().with_owner(true));
    let own = s
        .join(
            Participant::new("own", "own", "#123456", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    agent_write(&f, &s, bot);
    f.advance(1_000);
    app_save(&f.path(), PERSONS);
    let DiskCheck::Conflict { copy: Some(c1), .. } = s.file_changed().unwrap() else {
        return Err(format!("{ROW}: control: the first save keeps both"));
    };
    set_field(&s, bot, "s1", "title", "Agent 2").unwrap();
    assert!(
        s.flush(bot).unwrap().written,
        "control: the kit's second write"
    );
    f.advance(1_000);
    app_save(&f.path(), "s1\ttitle=A\ns2\ttitle=Person 2\n");
    let DiskCheck::Conflict { copy: Some(c2), .. } = s.file_changed().unwrap() else {
        return Err(format!("{ROW}: control: the second save keeps both"));
    };
    fs::write(&c2, "s1\ttitle=edited by a person\n").unwrap();
    let doc = f.store.find(&key()).unwrap().expect("the doc");
    assert_eq!(
        f.store.live_copies(doc).unwrap().len(),
        2,
        "control: both copies recorded"
    );
    let out = s
        .purge(own, PurgeRequest::new(PurgeScope::History))
        .map_err(|e| format!("{ROW}: the purge: {e}"))?;
    need(!c1.exists(), ROW, || {
        format!("the copy of erased content is deleted: {}", c1.display())
    })?;
    let rows = f.store.live_copies(doc).unwrap();
    need(rows.len() == 1 && c2.ends_with(&rows[0].1.rec), ROW, || {
        format!("only the edited copy's record stays: {rows:?}")
    })?;
    need(out.counts.conflict_copies == 1, ROW, || {
        format!("the purge counts the copy it removed: {:?}", out.counts)
    })?;
    need(c2.exists(), ROW, || "the edited copy is left".into())?;
    need(
        out.not_reached
            .iter()
            .any(|n| matches!(n, NotReached::ConflictCopies(1))),
        ROW,
        || format!("the edited copy is reported: {:?}", out.not_reached),
    )
}

#[test]
fn keyed_purge_removes_live_conflict_copies() {
    row_keyed_purge_removes_live_conflict_copies().unwrap();
}

/// The planted fault "purge skips live copies": the purge row goes red, by name.
#[test]
fn the_planted_purge_skips_live_copies_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::PurgeSkipsLiveCopies, || {
        let name = "row_keyed_purge_removes_live_conflict_copies";
        let e = row_keyed_purge_removes_live_conflict_copies()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
    });
}

/// Unchanged for tables: an outside save over a kit write merges by row and column, as
/// before (the grid's three-way merge; no copy, no conflict event).
#[test]
fn tables_keep_their_outside_save_merge() {
    let f = fx("table");
    let path = f.ws.join("t.csv");
    fs::write(&path, "a,b\nc,d\n").unwrap();
    let s = f
        .mgr
        .open_file(
            Arc::new(grid_pack::GridLite),
            &DocKey {
                space: "ws".into(),
                path: "t.csv".into(),
            },
            &path,
            &f.ws,
        )
        .unwrap();
    let p = f.person(&s);
    let r = s
        .edit_ops_with(p, &EditOptions::new(), |v| {
            let mut t = v.table().map_err(|e| e.to_string())?;
            t.set_cell(&k("r1"), &k("c1"), "A")
                .map_err(|e| e.to_string())
        })
        .unwrap();
    assert!(r.is_some());
    assert!(
        s.flush(p).unwrap().written,
        "control: the kit wrote the table"
    );
    assert_ne!(fs::read_to_string(&path).unwrap(), "a,b\nc,d\n");
    // the app's buffer from before the kit's write, with another cell changed
    f.advance(1_000);
    app_save(&path, "a,b\nc,D\n");
    let out = s.file_changed().unwrap();
    assert!(matches!(out, DiskCheck::Ingested { .. }), "{out:?}");
    assert!(
        conflict_events(&f).is_empty(),
        "no conflict event for a table"
    );
    assert_eq!(names_in(&f.ws), ["t.csv"], "no conflict copy for a table");
}

/// The planted fault "keyed outside save applied silently": each stale-save row goes red, by
/// name.
#[test]
fn the_planted_keyed_outside_save_applied_silently_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::OutsideSaveAppliedSilently, || {
        for (name, row) in [
            (
                "row_keyed_stale_save_after_a_kit_write_keeps_both",
                row_keyed_stale_save_after_a_kit_write_keeps_both as fn() -> Result<(), String>,
            ),
            (
                "row_keyed_stale_save_after_a_kit_write_across_a_restart",
                row_keyed_stale_save_after_a_kit_write_across_a_restart,
            ),
            (
                "row_keyed_save_on_unwritten_changes_keeps_both",
                row_keyed_save_on_unwritten_changes_keeps_both,
            ),
        ] {
            let e = row().expect_err(&format!("planted, {name} must fail"));
            assert!(e.starts_with(&format!("{name}: ")), "{e}");
        }
    });
}
