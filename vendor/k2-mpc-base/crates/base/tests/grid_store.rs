//! C1c: sessions and the store for structured (grid) docs, public API only. A tiny table pack
//! (`csv-lite`: one record per line, fields split by `,`, no quoting) stands in for the CSV
//! pack. Test names carry the amendment's IT names where one applies (`store::…`,
//! `versions::…`, `blame::…`, `grid::…`).
//!
//! - the store: typed versions round-trip; a store without grid docs stays at format 5 (its
//!   tables and stamp untouched); the first grid doc upgrades it to 6 in place; a newer
//!   store is refused with one line;
//! - sessions: structured edits (A6) with their stream (A5), counts and the read's content;
//!   an agent needs a base, checked per facet (K-BR-12); the size limits (K-CSV-7, KC4);
//!   `rows drop-deleted` (owner only, frees the cap, history keeps the rows);
//! - history: restore by identity, revert from v3 records (with conflicts and U23 blame),
//!   `blame_cell`, change summaries;
//! - files: an outside edit applied by key (keys kept, unsynced changes kept); `write --base`
//!   for grids with the conflict context; hidden cell writes name the deleter.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::doc::{Doc, Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Applied, Key, ListOp, MapOp, Value};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    AuditEvent, AuditViolation, ChangeKind, Config, ConnId, Declined, EditOptions, GridWrite,
    GridWriteOptions, Host, LogItem, LogQuery, Need, Pack, PackContent, PackError, Participant,
    ParticipantKind, Rights, Session, SessionError, SessionManager, Stay, TextFilePack, UndoMode,
    UndoOutcome, UndoRequest, View,
};
use k2_mpc_base::store::{
    DocKey, DocStore, StoreError, VersionReason, STORE_FORMAT, STORE_FORMAT_BASE, STORE_FORMAT_READ,
};
use k2_mpc_base::table::{self, Field, Record, Shape, Table, TableError, CELLS, ROWS};
use k2_mpc_base::tools::{exit, session_exit};

// ---------- the test pack ----------

struct Lite;
const LITE: Schema = Schema {
    name: "csv-lite",
    roots: table::ROOTS,
};

fn records(s: &str) -> Vec<Record> {
    s.lines()
        .map(|l| {
            if l.is_empty() {
                Vec::new()
            } else {
                l.split(',').map(Field::new).collect()
            }
        })
        .collect()
}

impl Pack for Lite {
    fn schema(&self) -> Schema {
        LITE
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let s = std::str::from_utf8(b).map_err(|_| Declined {
            code: "not_utf8",
            message: "not UTF-8".into(),
        })?;
        let recs = records(s);
        let (_, edits) =
            Table::import(&recs, false, Shape::detect(&recs)).map_err(|e| Declined {
                code: "malformed",
                message: e.to_string(),
            })?;
        let mut d = Doc::new(LITE).unwrap();
        d.edit_as(2, &edits).unwrap();
        Ok(PackContent {
            roots: d.export_neutral().unwrap(),
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let t = View::of_content(c)
            .table()
            .map_err(|e| PackError(e.to_string()))?;
        let mut out = String::new();
        for rec in t.render() {
            let f: Vec<&str> = rec.iter().map(|f| f.value.as_str()).collect();
            out.push_str(&f.join(","));
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

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
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct TmpDir(std::path::PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "k2mb-grid-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    dir: TmpDir,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    mgr: SessionManager,
}

fn config() -> Config {
    let mut c = Config::default();
    c.edit_burst = 100_000;
    c.edit_rate_per_s = 100_000;
    c
}

fn fx_with(tag: &str, c: Config) -> Fx {
    let dir = TmpDir::new(tag);
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), c);
    Fx {
        dir,
        host,
        store,
        mgr,
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, config())
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn participant(pid: &str, kind: ParticipantKind) -> Participant {
    Participant::new(pid, pid.to_uppercase(), "#123456", kind, "cli")
}

impl Fx {
    fn open(&self, path: &str, body: &str) -> Arc<Session> {
        self.mgr
            .open(Arc::new(Lite), &key(path), Some(body.as_bytes()))
            .unwrap()
    }
    fn join(&self, s: &Session, pid: &str, kind: ParticipantKind, r: Rights) -> ConnId {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
        s.join(participant(pid, kind), Stay::Held).unwrap().conn
    }
    fn person(&self, s: &Session, pid: &str) -> ConnId {
        self.join(s, pid, ParticipantKind::Person, Rights::full())
    }
    fn agent(&self, s: &Session, pid: &str) -> ConnId {
        self.join(s, pid, ParticipantKind::Agent, Rights::full())
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn text(s: &Session, c: ConnId) -> String {
    String::from_utf8(s.read(c).unwrap().bytes).unwrap()
}

/// `MPC_MERGE_MUTANT` runs `f` with a merge fault planted on this thread
/// (`k2_mpc_base::differential::MergeMutant`, the `differential` feature, on for these tests),
/// so the row-pairing bar and its named tests can be shown to catch each one:
/// `outside-whole-row-live-cols` is mutant (xxxii), `by-cell-live-cols` is (xxxiii),
/// `write-base-neutral-conflicts` is (xxxiv), `whole-row-restores-dropped` is (xxxv),
/// `rewritten-row-before-dropped` is (xxxvi), `keyed-write-under-unknown-key` is (xxxvii).
/// Unset, `f`
/// runs as is. Any other value panics.
fn under_merge_mutant<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_merge_mutant, MergeMutant};
    match std::env::var("MPC_MERGE_MUTANT").as_deref() {
        Ok("outside-whole-row-live-cols") => {
            with_merge_mutant(MergeMutant::OutsideWholeRowLiveCols, f)
        }
        Ok("by-cell-live-cols") => with_merge_mutant(MergeMutant::ByCellLiveCols, f),
        Ok("write-base-neutral-conflicts") => {
            with_merge_mutant(MergeMutant::WriteBaseNeutralConflicts, f)
        }
        Ok("whole-row-restores-dropped") => {
            with_merge_mutant(MergeMutant::WholeRowRestoresDropped, f)
        }
        Ok("rewritten-row-before-dropped") => {
            with_merge_mutant(MergeMutant::RewrittenRowBeforeDropped, f)
        }
        Ok("keyed-write-under-unknown-key") => {
            with_merge_mutant(MergeMutant::KeyedWriteUnderUnknownKey, f)
        }
        Ok(other) => panic!("MPC_MERGE_MUTANT={other}: not a merge mutant"),
        Err(_) => f(),
    }
}

/// A random property's config (every random grid property uses it): the run's seed is
/// `PROPTEST_RNG_SEED` if set, else drawn here, and printed first, so a failure's captured
/// output names it and `PROPTEST_RNG_SEED=<seed>` replays the run exactly; failing cases persist
/// next to this file (`*.proptest-regressions`, proptest's own format) and run first next time.
fn seeded(cases: u32, name: &str) -> proptest::test_runner::Config {
    let seed = std::env::var("PROPTEST_RNG_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            use std::hash::{BuildHasher, Hasher};
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos()),
            );
            h.finish()
        });
    println!("PROPTEST_RNG_SEED={seed} replays {name}");
    let mut c = proptest::test_runner::Config::with_cases(cases);
    c.rng_seed = proptest::test_runner::RngSeed::Fixed(seed);
    c.source_file = Some(file!());
    c
}

/// Run a table op on the live table, as `c`, with `opts`.
fn op(
    s: &Session,
    c: ConnId,
    opts: &EditOptions,
    f: impl FnOnce(&mut Table) -> Result<Vec<Edit>, TableError>,
) -> Result<u64, SessionError> {
    match s.edit_ops_with(c, opts, |v| {
        let mut t = v.table().map_err(|e| e.to_string())?;
        f(&mut t).map_err(|e| e.to_string())
    }) {
        Ok(r) => Ok(r.map_or(0, |r| r.version)),
        Err(k2_mpc_base::session::EditWithError::Session(e)) => Err(e),
        Err(k2_mpc_base::session::EditWithError::Compute(e)) => panic!("table op: {e}"),
    }
}

fn person_set(s: &Session, c: ConnId, r: &str, col: &str, v: &str) -> u64 {
    op(s, c, &EditOptions::new(), |t| t.set_cell(&k(r), &k(col), v)).unwrap()
}

/// A raw store reader: the format stamp and the table names.
fn raw(store: &std::path::Path) -> (u64, Vec<String>) {
    use redb::{Database, TableDefinition, TableHandle};
    const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
    let d = Database::open(store).unwrap();
    let r = d.begin_read().unwrap();
    let f = r.open_table(META).unwrap();
    let v = f.get("store_format").unwrap().unwrap().value();
    let mut names: Vec<String> = r
        .list_tables()
        .unwrap()
        .map(|t| t.name().to_owned())
        .collect();
    names.sort();
    (v, names)
}

// ---------- the store: format 6, lazily ----------

#[test]
fn store_v5_untouched_without_grid_docs() {
    let f = fx("v5");
    let s = f
        .mgr
        .open(Arc::new(MD), &key("a.md"), Some(b"one\n"))
        .unwrap();
    let c = f.person(&s, "p");
    s.replace_text(c, TextFilePack::ROOT, "two\n").unwrap();
    s.name_version(c, "v", None).unwrap();
    let path = f.dir.0.join("docs.redb");
    let Fx { mgr, store, .. } = f;
    mgr.release().unwrap();
    drop((mgr, store, s));
    let (v, names) = raw(&path);
    assert_eq!(v, STORE_FORMAT_BASE, "a text-only store stays at format 5");
    assert!(
        !names.iter().any(|n| n == "elem_writes"),
        "no format-6 table: {names:?}"
    );
    // reopening by this build changes nothing either
    let s2 = DocStore::open(&path).unwrap();
    assert_eq!(s2.format().unwrap(), STORE_FORMAT_BASE);
    drop(s2);
    assert_eq!(raw(&path), (v, names));
}

#[test]
fn store_upgrades_to_6_on_first_grid_doc() {
    let f = fx("v6");
    assert_eq!(f.store.format().unwrap(), STORE_FORMAT_BASE);
    f.open("t.csv", "a,b\nc,d\n");
    assert_eq!(f.store.format().unwrap(), STORE_FORMAT);
    let path = f.dir.0.join("docs.redb");
    let Fx { mgr, store, .. } = f;
    mgr.release().unwrap();
    drop((mgr, store));
    let (v, names) = raw(&path);
    assert_eq!(v, 6);
    assert!(names.iter().any(|n| n == "elem_writes"));
}

#[test]
fn store_newer_format_refused_with_one_line() {
    let f = fx("newer");
    let path = f.dir.0.join("docs.redb");
    drop(f.mgr);
    drop(f.store);
    {
        use redb::{Database, TableDefinition};
        const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
        let d = Database::open(&path).unwrap();
        let w = d.begin_write().unwrap();
        w.open_table(META)
            .unwrap()
            .insert("store_format", STORE_FORMAT_READ + 1)
            .unwrap();
        w.commit().unwrap();
    }
    // v0.10.1 reads format 7 (the native engine's, forward-declared): the first format it
    // refuses is 8
    let e = DocStore::open(&path).err().expect("refused");
    assert_eq!(
        e,
        StoreError::Newer {
            found: STORE_FORMAT_READ + 1
        }
    );
    let line = e.to_string();
    assert!(
        !line.contains('\n') && line.contains("this build reads up to 7"),
        "{line}"
    );
}

#[test]
fn store_typed_versions_round_trip() {
    let f = fx("typed");
    let s = f.open("t.csv", "a,b\nc,d\n");
    let c = f.person(&s, "p");
    person_set(&s, c, "r1", "c2", "B");
    op(&s, c, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
    let vid = s.name_version(c, "named", None).unwrap();
    let shown = s.show_content(c, vid).unwrap();
    // the version is the content readers saw, by key, with the codec settings
    assert_eq!(shown, s.read(c).unwrap().content);
    let v = f
        .store
        .version(f.store.find(&key("t.csv")).unwrap().unwrap(), vid)
        .unwrap();
    assert_eq!(v.codec, Some(vec![1]));
    let rows = shown.iter().find(|(r, _)| r == ROWS).unwrap();
    assert_eq!(rows.1, Neutral::List(vec![(k("r1"), Value::Null)]));
    // and survives a restart
    let path = f.dir.0.join("docs.redb");
    let Fx { mgr, store, .. } = f;
    mgr.release().unwrap();
    drop((mgr, store, s));
    let st = DocStore::open(&path).unwrap();
    let id = st.find(&key("t.csv")).unwrap().unwrap();
    assert_eq!(st.version(id, vid).unwrap().content, shown);
}

// ---------- sessions: edits, stream, counts ----------

#[test]
fn grid_edit_stream_counts_and_content() {
    let f = fx("edit");
    let s = f.open("t.csv", "a,b\nc,d\n");
    let c = f.person(&s, "p");
    let rx = s.subscribe(c).unwrap();
    let r = s
        .edit_ops(
            c,
            vec![Edit::Map {
                root: CELLS.into(),
                ops: vec![MapOp::Set {
                    key: table::cell_key(&k("r2"), &k("c1")),
                    value: Value::str("C"),
                }],
            }],
            &EditOptions::new(),
        )
        .unwrap();
    assert_eq!(text(&s, c), "a,b\nC,d\n");
    assert_eq!(r.elems.len(), 1);
    assert_eq!((r.elems[0].root.as_str(), r.elems[0].set), (CELLS, 1));
    let ev = rx.try_recv().unwrap();
    assert_eq!(ev.version, r.version);
    assert_eq!(ev.root, CELLS);
    match &ev.applied {
        Applied::Map { ops, .. } => {
            assert_eq!(ops[0].before, Some(Value::str("c")));
            assert_eq!(ops[0].after, Some(Value::str("C")));
        }
        other => panic!("{other:?}"),
    }
    // the log's summary counts elements
    let log = s.log(c, &LogQuery::default()).unwrap();
    let LogItem::Change { info, .. } = &log[0] else {
        panic!("{log:?}")
    };
    assert_eq!(info.seq, r.version);
    assert_eq!(info.elems[0].set, 1);
    // a text edit can't ride in a structured change, nor can a drop
    let mixed = s.edit_ops(
        c,
        vec![Edit::List {
            root: ROWS.into(),
            ops: vec![ListOp::Drop { key: k("r1") }],
        }],
        &EditOptions::new(),
    );
    assert!(matches!(mixed, Err(SessionError::BadEdit(_))));
    // undo by element (spec v1.10): the cell set comes back off
    assert!(matches!(s.undo(c, 1), Ok(UndoOutcome::Applied { .. })));
    assert_eq!(text(&s, c), "a,b\nc,d\n");
}

#[test]
fn grid_agent_edit_needs_base() {
    let f = fx("needsbase");
    let s = f.open("t.csv", "a,b\n");
    let a = f.agent(&s, "agent-a");
    let e = op(&s, a, &EditOptions::new(), |t| {
        t.set_cell(&k("r1"), &k("c1"), "x")
    })
    .unwrap_err();
    assert_eq!(e, SessionError::NeedsBase);
    assert_eq!(session_exit(&e), exit::USAGE);
    assert_eq!(
        e.to_string(),
        "an agent's grid edit needs a base: read first and pass --base"
    );
    // with a base it applies; a person needs none (K-BR-12, live last-writer-wins)
    let v = s.read(a).unwrap().version;
    op(&s, a, &EditOptions::new().base(v), |t| {
        t.set_cell(&k("r1"), &k("c1"), "x")
    })
    .unwrap();
    let p = f.person(&s, "p");
    person_set(&s, p, "r1", "c2", "y");
    assert_eq!(text(&s, p), "x,y\n");
}

#[test]
fn grid_base_conflict_per_facet() {
    let f = fx("facet");
    let s = f.open("t.csv", "a\nb\nc\n");
    let a = f.agent(&s, "agent-a");
    let b = f.agent(&s, "agent-b");
    let base = s.read(a).unwrap().version;
    // b writes another row and moves r3 after the base
    let vb = s.read(b).unwrap().version;
    op(&s, b, &EditOptions::new().base(vb), |t| {
        t.set_cell(&k("r2"), &k("c1"), "B")
    })
    .unwrap();
    let vb = s.read(b).unwrap().version;
    op(&s, b, &EditOptions::new().base(vb), |t| {
        t.move_row(&k("r3"), Anchor::Start)
    })
    .unwrap();
    // a write to another row since the base doesn't refuse a's cell set
    op(&s, a, &EditOptions::new().base(base), |t| {
        t.set_cell(&k("r1"), &k("c1"), "A")
    })
    .unwrap();
    // a move of the same row does
    let e = op(&s, a, &EditOptions::new().base(base), |t| {
        t.move_row(&k("r3"), Anchor::End)
    })
    .unwrap_err();
    let SessionError::FacetConflict(cs) = &e else {
        panic!("{e:?}")
    };
    assert_eq!(session_exit(&e), exit::NO);
    assert_eq!(cs[0].by.as_deref(), Some("agent-b"));
    assert_eq!((cs[0].root.as_str(), cs[0].key.as_str()), (ROWS, "r3"));
    assert_eq!(cs[0].now, "live at #1");
    // a later identical value by someone else still conflicts (write identity, not value)
    let base2 = s.read(a).unwrap().version;
    let p = f.person(&s, "p");
    // p sets the cell to the value it already holds: a write all the same (raw op: the
    // table's builders skip a set that changes nothing)
    let same = s
        .edit_ops(
            p,
            vec![Edit::Map {
                root: CELLS.into(),
                ops: vec![MapOp::Set {
                    key: table::cell_key(&k("r1"), &k("c1")),
                    value: Value::str("A"),
                }],
            }],
            &EditOptions::new(),
        )
        .unwrap();
    assert!(same.version > base2, "an identical value is still a write");
    let e = op(&s, a, &EditOptions::new().base(base2), |t| {
        t.set_cell(&k("r1"), &k("c1"), "Z")
    })
    .unwrap_err();
    assert!(matches!(e, SessionError::FacetConflict(_)), "{e:?}");
    // the caller's own writes since never refuse
    let base3 = s.read(a).unwrap().version;
    op(&s, a, &EditOptions::new().base(base3), |t| {
        t.set_cell(&k("r2"), &k("c1"), "a1")
    })
    .unwrap();
    op(&s, a, &EditOptions::new().base(base3), |t| {
        t.set_cell(&k("r2"), &k("c1"), "a2")
    })
    .unwrap();
    // a row delete checks the cells it hides: p writes r1's cell after base3
    person_set(&s, p, "r1", "c1", "P");
    let e = op(&s, a, &EditOptions::new().base(base3), |t| {
        t.delete_row(&k("r1"))
    })
    .unwrap_err();
    assert!(matches!(e, SessionError::FacetConflict(_)), "{e:?}");
}

// ---------- the size limits (K-CSV-7, KC4) and rows drop-deleted ----------

#[test]
fn grid_verb_past_cap_exit_2_and_drop_deleted_frees_it() {
    let mut c = config();
    c.grid_max_cells = 6;
    let f = fx_with("cap", c);
    let s = f.open("t.csv", "a,b\nc,d\ne,f\n");
    let p0 = f.join(
        &s,
        "own",
        ParticipantKind::Person,
        Rights::full().with_owner(true),
    );
    let p = f.person(&s, "p");
    // at the cap: growing is refused with the plain message; nothing is dropped
    let e = op(&s, p, &EditOptions::new(), |t| {
        t.insert_row(Anchor::End, &["g".into(), "h".into()])
            .map(|x| x.1)
    })
    .unwrap_err();
    assert_eq!(session_exit(&e), exit::USAGE);
    assert_eq!(
        e.to_string(),
        "this table is at its size limit (6 cells, deleted rows included); the owner can \
         free space with `rows drop-deleted`"
    );
    assert_eq!(text(&s, p), "a,b\nc,d\ne,f\n");
    // a change that doesn't grow it passes
    person_set(&s, p, "r1", "c1", "A");
    // deleted rows still count
    op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
    let still = op(&s, p, &EditOptions::new(), |t| {
        t.insert_row(Anchor::End, &["g".into()]).map(|x| x.1)
    });
    assert!(
        matches!(still, Err(SessionError::AtLimit { .. })),
        "{still:?}"
    );
    // only the owner may drop them (exit 3), and it is audited
    let e = s.drop_deleted(p).unwrap_err();
    assert_eq!(e, SessionError::Forbidden { need: Need::Owner });
    assert_eq!(session_exit(&e), exit::REFUSED);
    let before = s.read(p0).unwrap().version;
    let r = s.drop_deleted(p0).unwrap();
    assert!(r.version > before);
    assert_eq!(text(&s, p), "A,b\ne,f\n", "nothing shown changes");
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::DroppedDeleted { by, rows: 1, cols: 0, cells: 2, .. } if by == "own"
    )));
    // the cap frees
    op(&s, p, &EditOptions::new(), |t| {
        t.insert_row(Anchor::End, &["g".into(), "h".into()])
            .map(|x| x.1)
    })
    .unwrap();
    assert_eq!(text(&s, p), "A,b\ne,f\ng,h\n");
    // r2 can't come back from the sheet, and history keeps it
    let e = s.edit_ops(
        p,
        vec![Edit::List {
            root: ROWS.into(),
            ops: vec![ListOp::Restore { key: k("r2") }],
        }],
        &EditOptions::new(),
    );
    assert!(matches!(e, Err(SessionError::Doc(_))), "{e:?}");
    let import = s.versions(p0).unwrap()[0].id;
    let v = s.show_content(p0, import).unwrap();
    let Neutral::List(rows) = &v.iter().find(|(r, _)| r == ROWS).unwrap().1 else {
        panic!()
    };
    assert!(rows.iter().any(|(r, _)| r.as_str() == "r2"));
    // the log names it as the owner's change
    let log = s.log(p0, &LogQuery::default().by("own")).unwrap();
    assert!(
        matches!(&log[0], LogItem::Change { info, .. } if info.elems.iter().any(|c| c.dropped == 1))
    );
}

#[test]
fn grid_import_over_the_cap_is_declined_too_large() {
    let mut c = config();
    c.grid_max_cells = 3;
    let f = fx_with("toolarge", c);
    let e = f
        .mgr
        .open(Arc::new(Lite), &key("t.csv"), Some(b"a,b\nc,d\n"))
        .err()
        .expect("declined");
    let SessionError::Declined(d) = e else {
        panic!("{e:?}")
    };
    assert_eq!(d.code, "too_large");
    assert!(f.store.find(&key("t.csv")).unwrap().is_none());
    assert_eq!(
        f.store.format().unwrap(),
        STORE_FORMAT_BASE,
        "nothing stored"
    );
}

// ---------- history: restore, revert, blame ----------

#[test]
fn versions_restore_by_identity() {
    let f = fx("restore");
    let s = f.open("t.csv", "a,b\nc,d\ne,f\n");
    let p = f.person(&s, "p");
    let v1 = s.name_version(p, "before", None).unwrap();
    // delete a row, move one, change a cell, add a row
    op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
    op(&s, p, &EditOptions::new(), |t| {
        t.move_row(&k("r3"), Anchor::Start)
    })
    .unwrap();
    person_set(&s, p, "r2", "c2", "D");
    op(&s, p, &EditOptions::new(), |t| {
        t.insert_row(Anchor::End, &["x".into(), "y".into()])
            .map(|x| x.1)
    })
    .unwrap();
    assert_eq!(text(&s, p), "e,f\nc,D\nx,y\n");
    let r = s.restore(p, v1).unwrap();
    assert_eq!(text(&s, p), "a,b\nc,d\ne,f\n");
    // r1 came back under its key (revived, not re-created): its cells with it
    let view = s.read(p).unwrap().content;
    let Neutral::List(rows) = &view.iter().find(|(r, _)| r == ROWS).unwrap().1 else {
        panic!()
    };
    let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["r1", "r2", "r3"]);
    let log = s.log(p, &LogQuery::default()).unwrap();
    let LogItem::Change { info, .. } = log
        .iter()
        .find(|i| matches!(i, LogItem::Change { info, .. } if info.seq == r.version))
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(info.kind, ChangeKind::Restore);
    assert_eq!(info.of, Some(v1));
    assert_eq!(
        info.elems.iter().find(|c| c.root == ROWS).unwrap().restored,
        1
    );
    // a version of the doc as it was is cut first
    assert!(s
        .versions(p)
        .unwrap()
        .iter()
        .any(|v| v.reason == VersionReason::BeforeRestore));
}

#[test]
fn revert_by_key_and_restored_cell_shows_original() {
    let f = fx("revert");
    let s = f.open("t.csv", "a,b\nc,d\n");
    let p = f.person(&s, "p");
    let q = f.person(&s, "q");
    let set_p = person_set(&s, p, "r1", "c1", "A");
    let set_q = person_set(&s, q, "r1", "c1", "AA");
    // blame: q's write
    let b = s.blame_cell(p, &k("r1"), &k("c1")).unwrap().unwrap();
    assert_eq!((b.author.as_str(), b.change), ("q", set_q));
    // reverting q's set puts p's value back, and blame then shows p's original write
    let r = s.revert(q, set_q, false).unwrap();
    assert!(matches!(r, UndoOutcome::Applied { .. }), "{r:?}");
    assert_eq!(text(&s, p), "A,b\nc,d\n");
    let b = s.blame_cell(p, &k("r1"), &k("c1")).unwrap().unwrap();
    assert_eq!((b.author.as_str(), b.change), ("p", set_p));
    assert_eq!(b.restored_by.as_deref(), Some("q"));
    assert!(b.restored_in.is_some());
    // a delete reverts to a restore of the same key
    let del = op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
    s.revert(p, del, false).unwrap();
    assert_eq!(text(&s, p), "A,b\nc,d\n");
    // a move goes back after its old left neighbour
    let mv = op(&s, p, &EditOptions::new(), |t| {
        t.move_row(&k("r1"), Anchor::End)
    })
    .unwrap();
    assert_eq!(text(&s, p), "c,d\nA,b\n");
    s.revert(p, mv, false).unwrap();
    assert_eq!(text(&s, p), "A,b\nc,d\n");
    // someone else wrote it since: a conflict, nothing applied; --partial applies the rest
    let two = s
        .edit_ops(
            p,
            vec![Edit::Map {
                root: CELLS.into(),
                ops: vec![
                    MapOp::Set {
                        key: table::cell_key(&k("r1"), &k("c2")),
                        value: Value::str("B"),
                    },
                    MapOp::Set {
                        key: table::cell_key(&k("r2"), &k("c2")),
                        value: Value::str("D"),
                    },
                ],
            }],
            &EditOptions::new(),
        )
        .unwrap()
        .version;
    person_set(&s, q, "r2", "c2", "DD");
    let r = s.revert(p, two, false).unwrap();
    let UndoOutcome::Conflicts { conflicts, .. } = &r else {
        panic!("{r:?}")
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicts[0].key.as_deref(),
        Some(table::cell_key(&k("r2"), &k("c2")).as_str())
    );
    assert_eq!(conflicts[0].lines, (2, 2), "its row's rendered line");
    assert!(conflicts[0].theirs.starts_with("q in change"));
    assert_eq!(text(&s, p), "A,B\nc,DD\n");
    let r = s.revert(p, two, true).unwrap();
    let UndoOutcome::Applied { skipped, .. } = &r else {
        panic!("{r:?}")
    };
    assert_eq!(skipped.len(), 1);
    assert_eq!(text(&s, p), "A,b\nc,DD\n");
}

#[test]
fn revert_works_after_a_restart() {
    let f = fx("revert-restart");
    let s = f.open("t.csv", "a\nb\n");
    let p = f.person(&s, "p");
    let ch = op(&s, p, &EditOptions::new(), |t| {
        t.move_row(&k("r2"), Anchor::Start)
    })
    .unwrap();
    drop(s);
    f.mgr.release().unwrap();
    let path = f.dir.0.join("docs.redb");
    let Fx {
        host,
        store,
        mgr,
        dir: _dir,
    } = f;
    drop((mgr, store));
    let store = Arc::new(DocStore::open(&path).unwrap());
    let mgr = SessionManager::new(store, host.clone(), config());
    let s = mgr.open(Arc::new(Lite), &key("t.csv"), None).unwrap();
    let p = s
        .join(participant("p", ParticipantKind::Person), Stay::Held)
        .unwrap()
        .conn;
    s.revert(p, ch, false).unwrap();
    assert_eq!(text(&s, p), "a\nb\n");
}

// ---------- files: outside edits by key, write --base, hidden writes ----------

#[test]
fn outside_edit_is_applied_by_key_and_keeps_unsynced_changes() {
    let mut c = config();
    c.write_quiet_ms = 1_000_000;
    c.write_max_delay_ms = 1_000_000;
    let f = fx_with("disk", c);
    let dir = f.dir.0.join("files");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("t.csv");
    std::fs::write(&file, "a,b\nc,d\ne,f\n").unwrap();
    let s = f
        .mgr
        .open_file(Arc::new(Lite), &key("t.csv"), &file, &dir)
        .unwrap();
    let p = f.person(&s, "p");
    // a live change the file hasn't got yet
    person_set(&s, p, "r3", "c2", "F");
    // the file is re-saved outside with a row moved, then again with another edited
    std::fs::write(&file, "e,f\na,b\nc,d\n").unwrap();
    s.file_changed().unwrap();
    std::fs::write(&file, "e,f\na,b\nc,D\n").unwrap();
    s.file_changed().unwrap();
    // keys kept (r3 moved, not re-created), the unsynced cell kept, the edit applied
    assert_eq!(text(&s, p), "e,F\na,b\nc,D\n");
    let rows = s.read(p).unwrap().content;
    let Neutral::List(rows) = &rows.iter().find(|(r, _)| r == ROWS).unwrap().1 else {
        panic!()
    };
    let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["r3", "r1", "r2"]);
    let log = s.log(p, &LogQuery::default()).unwrap();
    assert!(log
        .iter()
        .any(|i| matches!(i, LogItem::Change { info, .. } if info.kind == ChangeKind::External)));
    // the outside edit's rows are new keys, never ones the live doc used
    std::fs::write(&file, "e,F\na,b\nc,D\nnew,row\n").unwrap();
    s.file_changed().unwrap();
    let blame = s.blame_cell(p, &k("r4"), &k("c1")).unwrap().unwrap();
    assert_eq!(blame.author, k2_mpc_base::session::EXTERNAL_PID);
    // and the file gets the live doc back
    s.flush(p).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text(&s, p));
    assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
}

/// C1d part 2: ONE outside save that moves a row and edits another keeps both keys (was:
/// the edited row deleted and re-inserted under a new key, losing its blame).
#[test]
fn outside_save_moving_one_row_and_editing_another_keeps_both_keys() {
    let f = fx("move-edit");
    let dir = f.dir.0.join("files");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("t.csv");
    std::fs::write(&file, "a,b\nc,d\ne,f\n").unwrap();
    let s = f
        .mgr
        .open_file(Arc::new(Lite), &key("t.csv"), &file, &dir)
        .unwrap();
    let p = f.person(&s, "p1");
    std::fs::write(&file, "e,f\na,b\nc,D\n").unwrap();
    s.file_changed().unwrap();
    assert_eq!(text(&s, p), "e,f\na,b\nc,D\n");
    let rows = s.read(p).unwrap().content;
    let Neutral::List(rows) = &rows.iter().find(|(r, _)| r == ROWS).unwrap().1 else {
        panic!()
    };
    let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["r3", "r1", "r2"]);
    // r2's untouched cell keeps its blame; its edited one is the outside edit's
    let b = s.blame_cell(p, &k("r2"), &k("c2")).unwrap().unwrap();
    assert_eq!(b.author, k2_mpc_base::session::EXTERNAL_PID);
    let b = s.blame_cell(p, &k("r2"), &k("c1")).unwrap().unwrap();
    assert_eq!(b.kind, Some(ChangeKind::Import));
    assert!(s.audit().unwrap().is_clean());
}

#[test]
fn write_base_grid_merges_by_key_and_reports_context() {
    let f = fx("wbase");
    let s = f.open("t.csv", "a,b\nc,d\ne,f\n");
    let a = f.agent(&s, "agent-a");
    let p = f.person(&s, "p");
    let base = s.read(a).unwrap().version;
    // p changes r2's first cell and moves r3 to the top after a's read
    person_set(&s, p, "r2", "c1", "C");
    op(&s, p, &EditOptions::new(), |t| {
        t.move_row(&k("r3"), Anchor::Start)
    })
    .unwrap();
    // a's body also changes r2's other cell: a row both changed, through a text: one conflict
    // at the row, by whoever changed it, and nothing applied (contract §3.4)
    let r = s
        .write_base_grid(a, base, b"A,b\nc,D\ne,f\n", &GridWriteOptions::default())
        .unwrap();
    let GridWrite::Conflicts(cs) = &r.outcome else {
        panic!("{r:?}")
    };
    assert_eq!(cs.len(), 1, "{cs:?}");
    assert_eq!(cs[0].conflict.at, table::Region::Row(k("r2")));
    assert_eq!(cs[0].conflict.rule.as_str(), "edited_row");
    assert_eq!(cs[0].written_by.as_deref(), Some("p"));
    assert_eq!(text(&s, p), "e,f\na,b\nC,d\n", "nothing applied");
    // a's body changes only r1, which p left alone: merges cleanly by key
    let r = s
        .write_base_grid(a, base, b"A,b\nc,d\ne,f\n", &GridWriteOptions::default())
        .unwrap();
    assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
    assert_eq!(text(&s, p), "e,f\nA,b\nC,d\n");
    // a conflicting cell: reported with who wrote it, where it is and its line
    let base = s.read(a).unwrap().version;
    person_set(&s, p, "r1", "c2", "BB");
    let r = s
        .write_base_grid(a, base, b"e,f\nA,XX\nC,d\n", &GridWriteOptions::default())
        .unwrap();
    let GridWrite::Conflicts(cs) = &r.outcome else {
        panic!("{r:?}")
    };
    assert_eq!(r.exit(), exit::NO);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].written_by.as_deref(), Some("p"));
    assert_eq!(cs[0].index, Some(1));
    assert_eq!(cs[0].lines, Some(2..3));
    assert_eq!(text(&s, p), "e,f\nA,BB\nC,d\n", "nothing applied");
    // an unknown base expires
    let r = s
        .write_base_grid(a, 999, b"x\n", &GridWriteOptions::default())
        .unwrap();
    assert_eq!(r.outcome, GridWrite::BaseExpired);
    assert_eq!(r.exit(), exit::TOO_OLD);
}

#[test]
fn a_cell_written_into_a_deleted_row_names_the_deleter() {
    let f = fx("hidden");
    let s = f.open("t.csv", "a,b\nc,d\n");
    let p = f.person(&s, "p");
    let q = f.person(&s, "q");
    let del = op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
    let rx = s.subscribe(p).unwrap();
    // q writes into the deleted row (a stale writer): kept, hidden, reported
    let r = s
        .edit_ops(
            q,
            vec![Edit::Map {
                root: CELLS.into(),
                ops: vec![MapOp::Set {
                    key: table::cell_key(&k("r2"), &k("c1")),
                    value: Value::str("late"),
                }],
            }],
            &EditOptions::new(),
        )
        .unwrap();
    assert_eq!(r.hidden.len(), 1);
    assert_eq!(r.hidden[0].deleted_by.as_deref(), Some("p"));
    assert_eq!(r.hidden[0].deleted_in, Some(del));
    assert!(r.hidden[0].row_hidden);
    // the deleter's side: the event names it too
    let ev = rx.try_recv().unwrap();
    assert_eq!(ev.hidden, r.hidden);
    assert_eq!(text(&s, p), "a,b\n");
    // and a restore brings it back
    op(&s, p, &EditOptions::new(), |t| t.restore_row(&k("r2"))).unwrap();
    assert_eq!(text(&s, p), "a,b\nlate,d\n");
}

#[test]
fn audit_is_clean_after_every_kind_of_change() {
    let f = fx("audit");
    let s = f.open("t.csv", "a,b\nc,d\ne,f\n");
    let p = f.person(&s, "p");
    let own = f.join(
        &s,
        "own",
        ParticipantKind::Person,
        Rights::full().with_owner(true),
    );
    person_set(&s, p, "r1", "c1", "A");
    let mv = op(&s, p, &EditOptions::new(), |t| {
        t.move_row(&k("r3"), Anchor::Start)
    })
    .unwrap();
    op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
    s.revert(p, mv, false).unwrap();
    s.drop_deleted(own).unwrap();
    let v = s.versions(p).unwrap()[0].id;
    s.restore(p, v).unwrap();
    let rep = s.audit().unwrap();
    assert!(rep.is_clean(), "{rep:?}");
    assert!(rep.checked > 10);
    assert!(!rep.violations.contains(&AuditViolation::LogReplay));
}

#[test]
fn line_map_and_reconcile_defaults() {
    let v = View::of_content(&Lite.decode(b"a,b\nc,d\n").unwrap().roots);
    let m = Lite.line_map(&v, &[1]).unwrap();
    assert_eq!(m.row_at(2), Some(&k("r2")));
    // reconcile: an edited line keeps its key
    let r = Lite.reconcile(&v, b"a,b\nc,D\n", &[1]).unwrap();
    let mut t = v.table().unwrap();
    t.apply(&r.edits).unwrap();
    assert_eq!(t.cell(&k("r2"), &k("c2")), Some("D"));
    assert_eq!(r.codec, vec![1]);
    // a text pack maps no lines
    let tv = View::of_content(&MD.decode(b"x\n").unwrap().roots);
    assert!(MD.line_map(&tv, &[]).is_none());
    let tr = MD
        .reconcile(&tv, b"y\n", &MD.decode(b"x\n").unwrap().codec)
        .unwrap();
    assert_eq!(tr.edits.len(), 1);
}

/// A text pack's reconcile pairs whole lines first (`lines::edit_hunks`, v0.9.7.1's one
/// pairing rule): deleting a line next to one that starts the same way deletes that line,
/// never its tail and the next line's head (the character-trimmed diff deleted `2\nseed:`
/// from `seed:1` / `seed:2` / `seed:3`), and a line rewritten between two lines that start
/// alike is that line's edit, not a splice of its neighbours.
#[test]
fn text_reconcile_pairs_whole_lines() {
    let cases: [(&str, &str); 3] = [
        ("seed:1\nseed:2\nseed:3\n", "seed:1\nseed:3\n"),
        ("p05:90\np26:95\np13:102\n", "p05:90\np20:90\np13:102\n"),
        ("ab\nabc\nab\n", "ab\nab\n"),
    ];
    for (old, new) in cases {
        let tv = View::of_content(&MD.decode(old.as_bytes()).unwrap().roots);
        let tr = MD
            .reconcile(
                &tv,
                new.as_bytes(),
                &MD.decode(old.as_bytes()).unwrap().codec,
            )
            .unwrap();
        let [Edit::Text { hunks, .. }] = &tr.edits[..] else {
            panic!("{old:?} -> {new:?}: one text edit, got {:?}", tr.edits);
        };
        let mut out = old.to_owned();
        for h in hunks.iter().rev() {
            // a hunk stays inside one line, or replaces whole lines: it never cuts across a
            // line break (the splice: one line's tail and the next line's head)
            let at_line_start = |at: usize| at == 0 || old.as_bytes()[at - 1] == b'\n';
            assert!(
                !old[h.start..h.end].contains('\n') && !h.insert.contains('\n')
                    || at_line_start(h.start) && at_line_start(h.end),
                "{old:?} -> {new:?}: hunk {h:?} cuts across a line break"
            );
            out.replace_range(h.start..h.end, &h.insert);
        }
        assert_eq!(out, new);
    }
}

/// The N−1 check's producer (`tools/n1/n1.sh`; run by hand with `K2MB_N1_OUT=<dir>`):
/// writes `text.redb` (Markdown docs only, edited and versioned by this build: it must stay
/// format 5 and open, read and write on the previous release) and `grid.redb` (with a grid
/// doc: format 6, which the previous release must refuse with one line).
#[test]
#[ignore]
fn n1_producer() {
    let out = std::path::PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    for (name, grid) in [("text.redb", false), ("grid.redb", true)] {
        let path = out.join(name);
        let _ = std::fs::remove_file(&path);
        let host = Arc::new(TestHost {
            now: AtomicU64::new(1_000_000),
            rights: Mutex::new(HashMap::new()),
            events: Mutex::new(Vec::new()),
        });
        host.rights
            .lock()
            .unwrap()
            .insert("p".into(), Rights::full().with_owner(true));
        host.rights
            .lock()
            .unwrap()
            .insert("bot".into(), Rights::full());
        let store = Arc::new(DocStore::open(&path).unwrap());
        let mgr = SessionManager::new(store.clone(), host.clone(), config());
        let s = mgr
            .open(Arc::new(MD), &key("a.md"), Some(b"one\n"))
            .unwrap();
        let c = s
            .join(participant("p", ParticipantKind::Person), Stay::Held)
            .unwrap()
            .conn;
        s.replace_text(c, TextFilePack::ROOT, "one\ntwo\n").unwrap();
        s.name_version(c, "n1", None).unwrap();
        s.revert(c, 2, false).unwrap();
        // tools/n1 `layout-both` (v0.10.1): revert the revert too, so the store holds restored
        // elements (provenance rows) and a revert record with roots for the other build to read
        if std::env::var("K2MB_N1_RESTORE").is_ok() {
            let last = s.read(c).unwrap().version;
            s.revert(c, last, false).unwrap();
        }
        // tools/n1 `layout-both` (after v0.10.2): an own replica and an agent join, so the store
        // holds `client_kinds` rows (a table a build before it doesn't know) for the other
        // build to open, write and refuse to rewrite
        if std::env::var("K2MB_N1_AGENTS").is_ok() {
            let r = s
                .join(
                    participant("bot", ParticipantKind::Agent).own_replica(),
                    Stay::Held,
                )
                .unwrap();
            let a = s
                .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                .unwrap();
            let id = store.find(&key("a.md")).unwrap().unwrap();
            let want: std::collections::HashSet<u64> = [a.client].into_iter().collect();
            assert_eq!(store.reusable_clients(id).unwrap(), want);
            println!(
                "{name}: client_kinds holds own replica {} and agent {}",
                r.client, a.client
            );
            // a second doc with both kinds: the consumer deletes a.md, this one stays
            let k = mgr
                .open(Arc::new(MD), &key("keep.md"), Some(b"keep\n"))
                .unwrap();
            k.join(
                participant("bot", ParticipantKind::Agent).own_replica(),
                Stay::Held,
            )
            .unwrap();
            k.join(participant("bot", ParticipantKind::Agent), Stay::Held)
                .unwrap();
        }
        if grid {
            let g = mgr
                .open(Arc::new(Lite), &key("t.csv"), Some(b"a,b\n"))
                .unwrap();
            let gc = g
                .join(participant("p", ParticipantKind::Person), Stay::Held)
                .unwrap()
                .conn;
            person_set(&g, gc, "r1", "c1", "A");
        }
        mgr.release().unwrap();
        drop((s, mgr));
        assert_eq!(
            store.format().unwrap(),
            if grid {
                STORE_FORMAT
            } else {
                STORE_FORMAT_BASE
            }
        );
    }
}

/// `client_kinds` rows as `(doc, client)`.
type KindRows = Vec<(u64, u64)>;

/// `client_kinds` as the file holds it, read without opening a `DocStore` (an open may change
/// it): (rows of deleted docs, rows of docs that exist).
fn raw_kinds(store: &std::path::Path) -> (KindRows, KindRows) {
    use redb::{Database, TableDefinition};
    const DOCS: TableDefinition<u64, &[u8]> = TableDefinition::new("docs");
    const KINDS: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("client_kinds");
    let d = Database::open(store).unwrap();
    let r = d.begin_read().unwrap();
    let docs = r.open_table(DOCS).unwrap();
    let kinds = match r.open_table(KINDS) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return (Vec::new(), Vec::new()),
        Err(e) => panic!("{e}"),
    };
    let (mut gone, mut live) = (Vec::new(), Vec::new());
    for kv in redb::ReadableTable::iter(&kinds).unwrap() {
        let (k, _) = kv.unwrap();
        if docs.get(k.value().0).unwrap().is_some() {
            live.push(k.value());
        } else {
            gone.push(k.value());
        }
    }
    (gone, live)
}

/// tools/n1 `node-both`, after the other build's consumer deleted a doc in the stores of
/// `K2MB_N1_OUT`: this build reads the file as that delete left it, then opens the store. No
/// row of a deleted doc may be left after this build's open, and no other doc's row taken.
#[test]
#[ignore = "tools/n1: reads the stores another build's consumer left"]
fn n1_kinds_after_delete() {
    let out = std::path::PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"));
    for name in ["text.redb", "grid.redb"] {
        let path = out.join(name);
        let (gone, live) = raw_kinds(&path);
        println!(
            "{name}: as the delete left it: client_kinds rows of deleted docs {}, of live docs {}",
            gone.len(),
            live.len()
        );
        drop(DocStore::open(&path).unwrap());
        let (gone_after, live_after) = raw_kinds(&path);
        println!(
            "{name}: after this build's open: client_kinds rows of deleted docs {}, of live docs {}",
            gone_after.len(),
            live_after.len()
        );
        assert_eq!(
            gone_after,
            vec![],
            "{name}: a deleted doc's rows survive an open"
        );
        assert_eq!(live_after, live, "{name}: a live doc's rows changed");
    }
}

/// Spec v1.10 S12: a version restore brings rows `drop-deleted` removed back under fresh
/// keys (a key is never reused in its causal past), cells re-keyed with them.
#[test]
fn restore_after_drop_deleted_mints_fresh_keys() {
    let f = fx("restore-drop");
    let s = f.open("t.csv", "a,b\nc,d\ne,f\n");
    let p0 = f.join(
        &s,
        "p0",
        ParticipantKind::Person,
        Rights::full().with_owner(true),
    );
    let v1 = s.name_version(p0, "before", None).unwrap();
    op(&s, p0, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
    s.drop_deleted(p0).unwrap();
    // drop is not erasure: the version still holds the row and its cells (negative test)
    let held = s.show_content(p0, v1).unwrap();
    let Neutral::Map(cells) = &held.iter().find(|(r, _)| r == CELLS).unwrap().1 else {
        panic!()
    };
    assert_eq!(
        cells.get(&table::cell_key(&k("r2"), &k("c1"))),
        Some(&Value::str("c"))
    );
    s.restore(p0, v1).unwrap();
    assert_eq!(text(&s, p0), "a,b\nc,d\ne,f\n");
    let content = s.read(p0).unwrap().content;
    let Neutral::List(rows) = &content.iter().find(|(r, _)| r == ROWS).unwrap().1 else {
        panic!()
    };
    let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["r1", "r4", "r3"], "r2 came back as a fresh key");
    // with its content
    let Neutral::Map(cells) = &content.iter().find(|(r, _)| r == CELLS).unwrap().1 else {
        panic!()
    };
    assert_eq!(
        cells.get(&table::cell_key(&k("r4"), &k("c1"))),
        Some(&Value::str("c"))
    );
    assert_eq!(
        cells.get(&table::cell_key(&k("r4"), &k("c2"))),
        Some(&Value::str("d"))
    );
    assert!(!cells.contains_key(&table::cell_key(&k("r2"), &k("c1"))));
    let b = s.blame_cell(p0, &k("r4"), &k("c1")).unwrap().unwrap();
    assert_eq!(b.kind, Some(ChangeKind::Restore));
    assert!(
        s.blame_cell(p0, &k("r2"), &k("c1"))
            .unwrap()
            .unwrap()
            .deleted
    );
    // the counters moved past it: the next row is new too
    op(&s, p0, &EditOptions::new(), |t| {
        t.insert_row(Anchor::End, &["g".into(), "h".into()])
            .map(|x| x.1)
    })
    .unwrap();
    let content = s.read(p0).unwrap().content;
    let Neutral::List(rows) = &content.iter().find(|(r, _)| r == ROWS).unwrap().1 else {
        panic!()
    };
    assert_eq!(rows.last().unwrap().0.as_str(), "r5");
    assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
}

/// Spec v1.10 S2: a write that puts a facet back holds it as the write it restores, on the
/// value, position and existence facets alike, so reverting an own chain newest first
/// applies at every step (no false conflict).
#[test]
fn revert_own_chain_newest_first_applies_on_every_facet() {
    let f = fx("lifo");
    let s = f.open("t.csv", "a,b\nc,d\ne,f\n");
    let p = f.person(&s, "p1");
    // value
    let b1 = person_set(&s, p, "r1", "c1", "A");
    let b2 = person_set(&s, p, "r1", "c1", "AA");
    assert!(matches!(
        s.revert(p, b2, false).unwrap(),
        UndoOutcome::Applied { .. }
    ));
    let r = s.revert(p, b1, false).unwrap();
    assert!(matches!(r, UndoOutcome::Applied { .. }), "value: {r:?}");
    assert_eq!(text(&s, p), "a,b\nc,d\ne,f\n");
    // position
    let b1 = op(&s, p, &EditOptions::new(), |t| {
        t.move_row(&k("r3"), Anchor::Start)
    })
    .unwrap();
    let b2 = op(&s, p, &EditOptions::new(), |t| {
        t.move_row(&k("r3"), Anchor::After(k("r1")))
    })
    .unwrap();
    assert_eq!(text(&s, p), "a,b\ne,f\nc,d\n");
    assert!(matches!(
        s.revert(p, b2, false).unwrap(),
        UndoOutcome::Applied { .. }
    ));
    assert_eq!(text(&s, p), "e,f\na,b\nc,d\n");
    let r = s.revert(p, b1, false).unwrap();
    assert!(matches!(r, UndoOutcome::Applied { .. }), "position: {r:?}");
    assert_eq!(text(&s, p), "a,b\nc,d\ne,f\n");
    // existence
    let b1 = op(&s, p, &EditOptions::new(), |t| {
        t.insert_row(Anchor::End, &["g".into(), "h".into()])
            .map(|x| x.1)
    })
    .unwrap();
    let b2 = op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r4"))).unwrap();
    assert!(matches!(
        s.revert(p, b2, false).unwrap(),
        UndoOutcome::Applied { .. }
    ));
    assert_eq!(text(&s, p), "a,b\nc,d\ne,f\ng,h\n");
    let r = s.revert(p, b1, false).unwrap();
    assert!(matches!(r, UndoOutcome::Applied { .. }), "existence: {r:?}");
    assert_eq!(text(&s, p), "a,b\nc,d\ne,f\n");
    // and someone else's write in the middle still conflicts
    let b1 = person_set(&s, p, "r2", "c2", "D");
    let q = f.person(&s, "p2");
    person_set(&s, q, "r2", "c2", "DD");
    assert!(matches!(
        s.revert(p, b1, false).unwrap(),
        UndoOutcome::Conflicts { .. }
    ));
    assert!(s.audit().unwrap().is_clean());
}

// ---------- C1e: undo by element (spec v1.10 §6a), one test per case class ----------

fn undone(r: Result<UndoOutcome, SessionError>) -> bool {
    matches!(r, Ok(UndoOutcome::Applied { ref skipped, .. }) if skipped.is_empty())
}

fn conflicted(r: Result<UndoOutcome, SessionError>) -> Vec<String> {
    match r {
        Ok(UndoOutcome::Conflicts { conflicts, .. }) | Ok(UndoOutcome::Blocked { conflicts }) => {
            conflicts.into_iter().filter_map(|c| c.why).collect()
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
}

fn raw_set(s: &Session, c: ConnId, r: &str, col: &str, v: &str) -> u64 {
    s.edit_ops(
        c,
        vec![Edit::Map {
            root: CELLS.into(),
            ops: vec![MapOp::Set {
                key: table::cell_key(&k(r), &k(col)),
                value: Value::str(v),
            }],
        }],
        &EditOptions::new(),
    )
    .unwrap()
    .version
}

#[test]
fn undo_class_identical_value_write_conflicts() {
    let f = fx("u-aba");
    let s = f.open("t.csv", "a,b\n");
    let (p1, p2) = (f.person(&s, "p1"), f.person(&s, "p2"));
    person_set(&s, p1, "r1", "c1", "A");
    raw_set(&s, p2, "r1", "c1", "A"); // the same value: a write all the same
    assert_eq!(conflicted(s.undo(p1, 1)), ["written since"]);
    assert_eq!(text(&s, p1), "A,b\n");
}

#[test]
fn undo_class_sort_vs_a_foreign_move() {
    let f = fx("u-sort");
    let s = f.open("t.csv", "3\n1\n2\n4\n");
    let (p1, p2) = (f.person(&s, "p1"), f.person(&s, "p2"));
    op(&s, p1, &EditOptions::new(), |t| {
        t.sort(&[table::SortKey::by(k("c1"))])
    })
    .unwrap();
    assert_eq!(text(&s, p1), "1\n2\n3\n4\n");
    // p2 moves a row the sort moved (r1, "3")
    op(&s, p2, &EditOptions::new(), |t| {
        t.move_row(&k("r1"), Anchor::End)
    })
    .unwrap();
    assert_eq!(text(&s, p1), "1\n2\n4\n3\n");
    // all or nothing: it conflicts, nothing applies
    assert_eq!(conflicted(s.undo(p1, 1)), ["moved since"]);
    assert_eq!(text(&s, p1), "1\n2\n4\n3\n");
    // --partial moves back the rows only p1 moved; r1 stays where p2 put it
    let partial = UndoRequest::steps(1).mode(UndoMode::Partial);
    let r = s.undo_with(p1, &partial).unwrap();
    assert!(
        matches!(&r, UndoOutcome::Applied { skipped, .. } if skipped.len() == 1),
        "{r:?}"
    );
    assert_eq!(text(&s, p1), "4\n3\n1\n2\n");
    // redo reapplies exactly what the undo applied, never the skipped row
    assert!(undone(s.redo(p1, 1)));
    assert_eq!(text(&s, p1), "1\n2\n4\n3\n");
    assert!(matches!(s.redo(p1, 1), Ok(UndoOutcome::Nothing)));
}

#[test]
fn undo_class_delete_then_foreign_cell_in_hidden_row() {
    let f = fx("u-hidden");
    let s = f.open("t.csv", "a,b\nc,d\n");
    let (p1, p2) = (f.person(&s, "p1"), f.person(&s, "p2"));
    op(&s, p1, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
    raw_set(&s, p2, "r2", "c1", "late");
    assert!(undone(s.undo(p1, 1)), "a delete's undo never conflicts");
    assert_eq!(
        text(&s, p1),
        "a,b\nlate,d\n",
        "the row comes back with p2's cell"
    );
    let b = s.blame_cell(p1, &k("r2"), &k("c1")).unwrap().unwrap();
    assert_eq!(b.author, "p2");
}

#[test]
fn undo_class_insert_then_foreign_cell_set() {
    let f = fx("u-ins");
    let s = f.open("t.csv", "a,b\n");
    let (p1, p2) = (f.person(&s, "p1"), f.person(&s, "p2"));
    op(&s, p1, &EditOptions::new(), |t| {
        t.insert_row(Anchor::End, &["x".into(), "y".into()])
            .map(|x| x.1)
    })
    .unwrap();
    raw_set(&s, p2, "r2", "c2", "Y");
    // p2's write holds the cell p1 set, and a cell keyed by her new row
    assert_eq!(
        conflicted(s.undo(p1, 1)),
        ["written since", "cell written since"]
    );
    assert_eq!(text(&s, p1), "a,b\nx,Y\n");
}

#[test]
fn undo_class_move_back_past_a_deleted_neighbour() {
    let f = fx("u-moveback");
    let s = f.open("t.csv", "a\nb\nc\nd\n");
    let (p1, p2) = (f.person(&s, "p1"), f.person(&s, "p2"));
    op(&s, p1, &EditOptions::new(), |t| {
        t.move_row(&k("r3"), Anchor::Start)
    })
    .unwrap();
    assert_eq!(text(&s, p1), "c\na\nb\nd\n");
    op(&s, p2, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
    assert!(undone(s.undo(p1, 1)));
    // back right after b, its left neighbour in place order, though b is deleted (S4,
    // v1.10 addendum)
    assert_eq!(text(&s, p1), "a\nc\nd\n");
    // so b comes back before c: the old order
    op(&s, p2, &EditOptions::new(), |t| t.restore_row(&k("r2"))).unwrap();
    assert_eq!(text(&s, p1), "a\nb\nc\nd\n");
}

#[test]
fn undo_class_own_lifo_chain() {
    let f = fx("u-lifo");
    let s = f.open("t.csv", "a\nb\nc\n");
    let p1 = f.person(&s, "p1");
    person_set(&s, p1, "r1", "c1", "A");
    person_set(&s, p1, "r1", "c1", "AA");
    op(&s, p1, &EditOptions::new(), |t| {
        t.move_row(&k("r3"), Anchor::Start)
    })
    .unwrap();
    op(&s, p1, &EditOptions::new(), |t| {
        t.move_row(&k("r3"), Anchor::End)
    })
    .unwrap();
    op(&s, p1, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
    for n in 0..5 {
        assert!(undone(s.undo(p1, 1)), "undo {n}");
    }
    assert_eq!(text(&s, p1), "a\nb\nc\n");
    // and all redone, and undone again as one request (all or nothing, --steps 5)
    let r = s.redo(p1, 5);
    assert!(undone(r.clone()), "{r:?}");
    assert_eq!(text(&s, p1), "AA\nc\n");
    assert!(undone(s.undo(p1, 5)));
    assert_eq!(text(&s, p1), "a\nb\nc\n");
}

#[test]
fn undo_class_own_later_write_off_the_stack_conflicts() {
    let f = fx("u-offstack");
    let s = f.open("t.csv", "a\n");
    let p1 = f.person(&s, "p1");
    person_set(&s, p1, "r1", "c1", "A");
    person_set(&s, p1, "r1", "c1", "AA");
    // the later batch leaves the stack unapplied (--drop): undo of the earlier would
    // overwrite p1's own later write, which she can no longer undo
    let drop = UndoRequest::steps(1).mode(UndoMode::Drop);
    assert!(matches!(
        s.undo_with(p1, &drop).unwrap(),
        UndoOutcome::Dropped { .. }
    ));
    assert_eq!(conflicted(s.undo(p1, 1)), ["written since"]);
    assert_eq!(text(&s, p1), "AA\n");
}

#[test]
fn undo_class_own_cell_clear_then_undo_applies() {
    let f = fx("u-clear");
    let s = f.open("t.csv", "a,b\n");
    let p1 = f.person(&s, "p1");
    op(&s, p1, &EditOptions::new(), |t| {
        t.clear_cell(&k("r1"), &k("c1"))
    })
    .unwrap();
    assert_eq!(text(&s, p1), ",b\n");
    assert!(undone(s.undo(p1, 1)));
    assert_eq!(text(&s, p1), "a,b\n");
    // a map delete (an absent cell) carries the batch's identity too (S2, R1)
    s.edit_ops(
        p1,
        vec![Edit::Map {
            root: CELLS.into(),
            ops: vec![MapOp::Delete {
                key: table::cell_key(&k("r1"), &k("c2")),
            }],
        }],
        &EditOptions::new(),
    )
    .unwrap();
    assert!(undone(s.undo(p1, 1)));
    assert_eq!(text(&s, p1), "a,b\n");
}

#[test]
fn undo_class_already_by_value() {
    let f = fx("u-already");
    let s = f.open("t.csv", "a,b\n");
    let (p1, p2) = (f.person(&s, "p1"), f.person(&s, "p2"));
    person_set(&s, p1, "r1", "c1", "A");
    let theirs = person_set(&s, p2, "r1", "c1", "a");
    let v = s.read(p1).unwrap().version;
    // the live value already is what the undo would write: `already`, nothing applies
    let r = s.undo(p1, 1).unwrap();
    assert!(
        matches!(&r, UndoOutcome::Applied { skipped, .. } if skipped.is_empty()),
        "{r:?}"
    );
    assert_eq!(s.read(p1).unwrap().version, v, "nothing written");
    assert!(
        matches!(s.redo(p1, 1), Ok(UndoOutcome::Nothing)),
        "nothing to redo"
    );
    let b = s.blame_cell(p1, &k("r1"), &k("c1")).unwrap().unwrap();
    assert_eq!(
        (b.author.as_str(), b.change),
        ("p2", theirs),
        "blame stays theirs"
    );
}

#[test]
fn undo_class_drop_between_batch_and_undo() {
    let f = fx("u-drop");
    let s = f.open("t.csv", "a\nb\n");
    let p0 = f.join(
        &s,
        "p0",
        ParticipantKind::Person,
        Rights::full().with_owner(true),
    );
    op(&s, p0, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
    s.drop_deleted(p0).unwrap();
    // drop-deleted is on no stack: the undo reaches the delete, whose row was dropped
    assert!(conflicted(s.undo(p0, 1)).contains(&"dropped".to_owned()));
    assert_eq!(text(&s, p0), "a\n");
}

/// S4's move-back anchor (v1.10 addendum): a move goes back after its left neighbour in place
/// order, deleted or not, so when that neighbour is restored every row is back at its index
/// from before the delete (K2: the order a person sees matches their mental model). With
/// the first wording (after the nearest live key) the restored row landed after the moved one.
#[test]
fn undo_class_move_back_then_restore_left_neighbour() {
    let f = fx("u-anchor");
    let s = f.open("t.csv", "a\nb\nc\nd\n");
    let (p1, p2) = (f.person(&s, "p1"), f.person(&s, "p2"));
    let o = EditOptions::new();
    let before = keys(&s, p1);
    op(&s, p2, &o, |t| t.delete_row(&k("r2"))).unwrap();
    op(&s, p1, &o, |t| t.move_row(&k("r3"), Anchor::Start)).unwrap();
    assert_eq!(text(&s, p1), "c\na\nd\n");
    assert!(undone(s.undo(p1, 1)), "own move back");
    assert_eq!(text(&s, p1), "a\nc\nd\n");
    assert!(undone(s.undo(p2, 1)), "the delete's undo");
    assert_eq!(
        keys(&s, p1),
        before,
        "every row at its index from before the delete"
    );
    assert_eq!(text(&s, p1), "a\nb\nc\nd\n");
}

/// The live row keys, in order.
fn keys(s: &Session, c: ConnId) -> Vec<String> {
    let content = s.read(c).unwrap().content;
    let Neutral::List(rows) = &content.iter().find(|(r, _)| r == ROWS).unwrap().1 else {
        panic!("rows is a list")
    };
    rows.iter().map(|(k, _)| k.as_str().to_owned()).collect()
}

/// v1.10 addendum (S4, G1-8 §3): restore → delete conflicts unless every undel covering one
/// of the element's deletes that isn't own is in the batch's causal past. A session's history
/// is linear, so the concurrent class is model-and-native only; this is its linear form on
/// Yrs: p2's later undel (covering a later delete) holds existence, and p1's undo of its own
/// restore conflicts instead of overriding it.
#[test]
fn undo_class_undels_covering_different_deletes() {
    let f = fx("u-undels");
    let s = f.open("t.csv", "a\nb\nc\n");
    let (p1, p2, p3, p4) = (
        f.person(&s, "p1"),
        f.person(&s, "p2"),
        f.person(&s, "p3"),
        f.person(&s, "p4"),
    );
    let o = EditOptions::new();
    op(&s, p3, &o, |t| t.delete_row(&k("r2"))).unwrap();
    op(&s, p1, &o, |t| t.restore_row(&k("r2"))).unwrap();
    op(&s, p4, &o, |t| t.delete_row(&k("r2"))).unwrap();
    op(&s, p2, &o, |t| t.restore_row(&k("r2"))).unwrap();
    assert_eq!(conflicted(s.undo(p1, 1)), ["existence written since"]);
    assert_eq!(text(&s, p1), "a\nb\nc\n", "p2's restore stands");
}

#[test]
fn undo_class_header_switch_is_one_region() {
    let f = fx("u-header");
    let s = f.open("t.csv", "h1,h2\na,b\n");
    let (p1, p2) = (f.person(&s, "p1"), f.person(&s, "p2"));
    op(&s, p1, &EditOptions::new(), |t| t.set_header(true)).unwrap();
    op(&s, p2, &EditOptions::new(), |t| {
        t.rename_col(&k("c1"), "H1")
    })
    .unwrap();
    // one region: p2's rename holds back the whole switch, even with --partial
    let partial = UndoRequest::steps(1).mode(UndoMode::Partial);
    let r = s.undo_with(p1, &partial).unwrap();
    assert!(matches!(r, UndoOutcome::Blocked { .. }), "{r:?}");
    assert_eq!(text(&s, p1), "H1,h2\na,b\n");
}

// ---------- files: a change that moves no byte, then an outside edit (v0.9.4's grid twin) ----------

/// The grid twin of disk.rs's `an_outside_edit_after_any_change_that_moved_no_byte_is_kept`
/// (v0.9.4): a grid doc, a change that leaves the file's bytes as they were (a header turned
/// on, or on and off; a cell set back to its value; an undo, a revert or a version restore of
/// a change), then nothing, a tick, a flush or a restart, then an outside edit of any line
/// (a cell, a new row, a row deleted, a row moved). The doc and the file must both end as
/// the outside edit, the sync audit (K-AUD-1) and the store audit must find nothing.
/// `NO_BYTE_CASES` sets the count (default 256; 10^4 on a node).
mod no_byte {
    use super::*;
    use k2_mpc_base::session::{DiskCheck, DiskState};
    use proptest::prelude::*;

    #[derive(Debug, Clone, Copy)]
    pub enum Change {
        HeaderOn,
        HeaderOnOff,
        CellSetBack,
        CellSetUndo,
        RowDeleteUndo,
        RowMoveUndo,
        Revert,
        Restore,
        /// A map delete of an empty cell: it only deletes, and moves no byte.
        UnsetEmpty,
        /// A row deleted and written back, then `rows drop-deleted`: it only deletes, and
        /// moves no byte.
        DropDeleted,
    }

    #[derive(Debug, Clone, Copy)]
    pub enum Between {
        Nothing,
        Tick,
        Flush,
        /// A crash: the manager is dropped without a write-back, and the doc reopened.
        Restart,
        FlushRestart,
    }

    #[derive(Debug, Clone, Copy)]
    pub enum Outside {
        Cell,
        Append,
        Delete,
        Move,
    }

    const VOCAB: [&str; 4] = ["a", "b", "a b", "3"];

    struct Fix {
        _dir: TmpDir,
        host: Arc<TestHost>,
        store: Arc<DocStore>,
        mgr: SessionManager,
        files: std::path::PathBuf,
    }

    impl Fix {
        fn new() -> Self {
            let dir = TmpDir::new("nobyte");
            let host = Arc::new(TestHost {
                now: AtomicU64::new(1_000_000),
                rights: Mutex::new(HashMap::new()),
                events: Mutex::new(Vec::new()),
            });
            host.rights
                .lock()
                .unwrap()
                .insert("p1".into(), Rights::full().with_owner(true));
            let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).unwrap());
            let mgr = SessionManager::new(store.clone(), host.clone(), config());
            let files = dir.0.join("files");
            std::fs::create_dir_all(&files).unwrap();
            Fix {
                _dir: dir,
                host,
                store,
                mgr,
                files,
            }
        }
        fn file(&self) -> std::path::PathBuf {
            self.files.join("t.csv")
        }
        fn open(&self) -> (Arc<Session>, ConnId) {
            let s = self
                .mgr
                .open_file(Arc::new(Lite), &key("t.csv"), &self.file(), &self.files)
                .unwrap();
            let c = s
                .join(participant("p1", ParticipantKind::Person), Stay::Held)
                .unwrap()
                .conn;
            (s, c)
        }
        fn advance(&self, ms: u64) {
            self.host.now.fetch_add(ms, Ordering::SeqCst);
        }
        fn tick(&self) {
            let r = self.mgr.tick().unwrap();
            assert!(r.errors.is_empty(), "tick errors: {:?}", r.errors);
        }
        /// A new manager on the same store; the old one is dropped without a write-back.
        fn restart(&mut self) {
            self.mgr = SessionManager::new(self.store.clone(), self.host.clone(), config());
        }
        fn bytes(&self) -> String {
            std::fs::read_to_string(self.file()).unwrap()
        }
    }

    fn disk(s: &Session) -> DiskState {
        s.status().unwrap().disk.expect("a file-backed session")
    }

    fn applied(r: UndoOutcome, why: &str) {
        assert!(
            matches!(r, UndoOutcome::Applied { ref skipped, .. } if skipped.is_empty()),
            "{why}: {r:?}"
        );
    }

    fn lines(rows: &[Vec<&str>]) -> String {
        rows.iter().map(|r| format!("{}\n", r.join(","))).collect()
    }

    pub fn case(
        seed: &[Vec<&'static str>],
        change: Change,
        at: (usize, usize),
        between: Between,
        outside: Outside,
        out_at: (usize, usize),
    ) {
        let why = format!("{change:?} then {between:?}, outside {outside:?}");
        let mut seed: Vec<Vec<&str>> = seed.to_vec();
        if matches!(change, Change::UnsetEmpty) {
            let (i, j) = (at.0 % seed.len(), at.1 % seed[0].len());
            seed[i][j] = "";
        }
        let mut seed_text = lines(&seed);
        let (mut nr, nc) = (seed.len(), seed[0].len());
        let row = |i: usize| k(&format!("r{}", i % nr + 1));
        let col = |j: usize| k(&format!("c{}", j % nc + 1));
        let mut f = Fix::new();
        std::fs::write(f.file(), &seed_text).unwrap();
        let (mut s, mut p) = f.open();
        let o = EditOptions::new();
        let (r, c) = (row(at.0), col(at.1));
        let other = |v: &str| VOCAB.iter().find(|x| **x != v).unwrap().to_string();
        let old = seed[at.0 % nr][at.1 % nc].to_owned();
        let old = old.as_str();
        match change {
            Change::HeaderOn => {
                op(&s, p, &o, |t| t.set_header(true)).unwrap();
            }
            Change::HeaderOnOff => {
                op(&s, p, &o, |t| t.set_header(true)).unwrap();
                op(&s, p, &o, |t| t.set_header(false)).unwrap();
            }
            Change::CellSetBack => {
                op(&s, p, &o, |t| t.set_cell(&r, &c, &other(old))).unwrap();
                op(&s, p, &o, |t| t.set_cell(&r, &c, old)).unwrap();
            }
            Change::CellSetUndo => {
                op(&s, p, &o, |t| t.set_cell(&r, &c, &other(old))).unwrap();
                applied(s.undo(p, 1).unwrap(), &why);
            }
            Change::RowDeleteUndo => {
                op(&s, p, &o, |t| t.delete_row(&r)).unwrap();
                applied(s.undo(p, 1).unwrap(), &why);
            }
            Change::RowMoveUndo => {
                // the last row to the start (two rows at least: the strategy's floor)
                let last = row(nr - 1);
                op(&s, p, &o, |t| t.move_row(&last, Anchor::Start)).unwrap();
                applied(s.undo(p, 1).unwrap(), &why);
            }
            Change::Revert => {
                let v = op(&s, p, &o, |t| t.set_cell(&r, &c, &other(old))).unwrap();
                applied(s.revert(p, v, false).unwrap(), &why);
            }
            Change::Restore => {
                let v = s.name_version(p, "before", None).unwrap();
                op(&s, p, &o, |t| t.set_cell(&r, &c, &other(old))).unwrap();
                s.restore(p, v).unwrap();
            }
            Change::UnsetEmpty => {
                s.edit_ops(
                    p,
                    vec![Edit::Map {
                        root: CELLS.into(),
                        ops: vec![MapOp::Delete {
                            key: table::cell_key(&r, &c),
                        }],
                    }],
                    &o,
                )
                .unwrap();
            }
            Change::DropDeleted => {
                op(&s, p, &o, |t| t.delete_row(&r)).unwrap();
                assert!(s.flush(p).unwrap().written, "{why}: the delete is written");
                seed.remove(at.0 % nr);
                nr -= 1;
                seed_text = lines(&seed);
                s.drop_deleted(p).unwrap();
            }
        }
        assert_eq!(text(&s, p), seed_text, "{why}: the change moved no byte");
        match between {
            Between::Nothing | Between::Restart => {}
            Between::Flush | Between::FlushRestart => {
                assert!(!s.flush(p).unwrap().written, "{why}: nothing to write");
                sync_audit(&s, change, &why);
            }
            Between::Tick => {
                f.advance(2_000);
                f.tick();
                sync_audit(&s, change, &why);
            }
        }
        if matches!(between, Between::Restart | Between::FlushRestart) {
            drop(s);
            f.restart();
            (s, p) = f.open();
            assert_eq!(disk(&s), DiskState::Clean, "{why}: reopened clean");
            sync_audit(&s, change, &why);
        }
        assert_eq!(f.bytes(), seed_text, "{why}: the file untouched");
        // a plain editor changes a line (the first may be the header)
        let mut rows: Vec<Vec<&str>> = seed.clone();
        let i = out_at.0 % nr;
        match outside {
            Outside::Cell => {
                let j = out_at.1 % nc;
                rows[i][j] = if rows[i][j] == "Q" { "R" } else { "Q" };
            }
            Outside::Append => rows.push(vec!["n"; nc]),
            Outside::Delete => {
                rows.remove(i);
            }
            Outside::Move => {
                let x = rows.remove(i);
                let to = out_at.1 % nr;
                rows.insert(to.min(rows.len()), x);
            }
        }
        let edited = lines(&rows);
        // what the kit must do: the outside edit aligned to the live table (the file held
        // it), keys kept, and every cell it didn't change keeps its blame
        let before = live_table(&s, p);
        let blame_before = blames(&s, p, &before);
        let decoded = Lite.decode(edited.as_bytes()).unwrap();
        let file_t = View::of_content(&decoded.roots).table().unwrap();
        let rec = table::reconcile(&before, &file_t.render()).unwrap();
        let mut want = before.clone();
        want.apply(&rec.edits).unwrap();
        f.advance(1_000);
        std::fs::write(f.file(), &edited).unwrap();
        let check = s.file_changed().unwrap();
        if edited != seed_text {
            assert!(
                matches!(check, DiskCheck::Ingested { .. }),
                "{why}: {check:?}"
            );
        }
        assert_eq!(text(&s, p), edited, "{why}: the doc is the outside edit");
        let after = live_table(&s, p);
        assert_eq!(after.live_rows(), want.live_rows(), "{why}: row keys");
        assert_eq!(after.live_cols(), want.live_cols(), "{why}: column keys");
        for (cell, author) in blames(&s, p, &after) {
            let kept = before.cell(&cell.0, &cell.1) == after.cell(&cell.0, &cell.1)
                && before.live_rows().contains(&cell.0);
            let want_author = match (kept, blame_before.get(&cell)) {
                (true, Some(a)) => a.clone(),
                _ => k2_mpc_base::session::EXTERNAL_PID.to_owned(),
            };
            assert_eq!(author, want_author, "{why}: blame of {cell:?}");
        }
        f.advance(2_000);
        f.tick();
        s.flush(p).unwrap();
        assert_eq!(f.bytes(), edited, "{why}: the file is the outside edit");
        assert_eq!(disk(&s), DiskState::Clean, "{why}");
        if edited == seed_text {
            // no outside edit came in: the replica is as the change left it
            sync_audit(&s, change, &format!("{why}: at the end"));
        } else {
            // the ingest made the replica the doc
            assert_eq!(
                s.audit_disk_sync().unwrap(),
                vec![],
                "{why}: K-AUD-1 at the end"
            );
        }
        assert!(s.audit().unwrap().is_clean(), "{why}: {:?}", s.audit());
    }

    /// K-AUD-1 before the outside save, for every change: the re-sync marker is the change
    /// counter (as v0.9.5 has it for text), so a change that only deleted (an empty cell
    /// unset, `drop-deleted`) re-syncs the replica too.
    fn sync_audit(s: &Session, change: Change, why: &str) {
        let _ = change;
        assert_eq!(s.audit_disk_sync().unwrap(), vec![], "{why}: K-AUD-1");
    }

    /// The change-counter marker (v0.9.5's, for grids in C1g): after a change that only
    /// deleted and moved no byte, the write-back that finds nothing to write re-syncs the
    /// replica (K-AUD-1 clean).
    #[test]
    fn the_replica_follows_a_change_that_only_deleted() {
        let f = Fix::new();
        std::fs::write(f.file(), "a,\nb,c\n").unwrap();
        let (s, p) = f.open();
        s.edit_ops(
            p,
            vec![Edit::Map {
                root: CELLS.into(),
                ops: vec![MapOp::Delete {
                    key: table::cell_key(&k("r1"), &k("c2")),
                }],
            }],
            &EditOptions::new(),
        )
        .unwrap();
        assert!(!s.flush(p).unwrap().written, "the unset moved no byte");
        assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
    }

    fn live_table(s: &Session, p: ConnId) -> Table {
        View::of_content(&s.read(p).unwrap().content)
            .table()
            .unwrap()
    }

    /// Every live cell's blame author.
    fn blames(s: &Session, p: ConnId, t: &Table) -> HashMap<(Key, Key), String> {
        let mut out = HashMap::new();
        for r in t.live_rows() {
            for c in t.live_cols() {
                if t.cell(&r, &c).is_some() {
                    if let Some(b) = s.blame_cell(p, &r, &c).unwrap() {
                        out.insert((r.clone(), c), b.author);
                    }
                }
            }
        }
        out
    }

    fn cases() -> u32 {
        std::env::var("NO_BYTE_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256)
    }

    fn grid() -> impl Strategy<Value = Vec<Vec<&'static str>>> {
        (1usize..4).prop_flat_map(|nc| {
            prop::collection::vec(
                prop::collection::vec(prop::sample::select(VOCAB.to_vec()), nc..=nc),
                2..7,
            )
        })
    }

    #[test]
    fn grid_outside_edit_after_any_change_that_moved_no_byte_is_kept() {
        let n = cases();
        let mut runner = proptest::test_runner::TestRunner::new(seeded(
            n,
            "grid_outside_edit_after_any_change_that_moved_no_byte_is_kept",
        ));
        let changes = prop::sample::select(vec![
            Change::HeaderOn,
            Change::HeaderOnOff,
            Change::CellSetBack,
            Change::CellSetUndo,
            Change::RowDeleteUndo,
            Change::RowMoveUndo,
            Change::Revert,
            Change::Restore,
            Change::UnsetEmpty,
            Change::DropDeleted,
        ]);
        let betweens = prop::sample::select(vec![
            Between::Nothing,
            Between::Tick,
            Between::Flush,
            Between::Restart,
            Between::FlushRestart,
        ]);
        let outsides = prop::sample::select(vec![
            Outside::Cell,
            Outside::Append,
            Outside::Delete,
            Outside::Move,
        ]);
        let strategy = (
            grid(),
            changes,
            (0usize..8, 0usize..4),
            betweens,
            outsides,
            (0usize..8, 0usize..8),
        );
        if let Err(e) = runner.run(&strategy, |(seed, change, at, between, outside, out_at)| {
            case(&seed, change, at, between, outside, out_at);
            Ok(())
        }) {
            panic!("{e}");
        }
        println!(
            "RUNREC {{\"job\":null,\"run\":\"nobyte_grid\",\"tool\":\"proptest\",\"status\":\"PASSED\",\"cases\":{n}}}"
        );
    }

    /// The sync audit sees a structured doc's stale replica (a planted violation): a cell the
    /// replica alone holds, while the doc is clean; a write-back with nothing to write heals it.
    #[test]
    fn the_sync_audit_reports_a_stale_grid_replica() {
        let f = Fix::new();
        std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
        let (s, p) = f.open();
        assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
        s.plant_replica_edits_for_tests(&[Edit::Map {
            root: CELLS.into(),
            ops: vec![MapOp::Set {
                key: table::cell_key(&k("r1"), &k("c1")),
                value: Value::str("z"),
            }],
        }])
        .unwrap();
        assert_eq!(
            s.audit_disk_sync().unwrap(),
            vec![k2_mpc_base::session::SyncDrift::Elements { root: CELLS.into() }]
        );
        // a header switch moves no byte; the write-back that finds nothing to write re-syncs
        op(&s, p, &EditOptions::new(), |t| t.set_header(true)).unwrap();
        assert!(!s.flush(p).unwrap().written);
        assert_eq!(s.audit_disk_sync().unwrap(), vec![], "re-synced");
    }
}

/// Outside saves as a keyed three-way merge (C1 must-have (c), contract §2.9.1): base = the
/// grid as the file held it, ours = the file, theirs = live (dirty or not); where both
/// changed one element the file wins, and the live value is never lost (a version cut, a
/// heads-up note to its author, an audit event). The grid twin of v0.9.5's outside-save
/// property, with the byte-neutral changes and an unwritten edit before the save.
mod outside_merge {
    use super::*;
    use k2_mpc_base::session::{DiskCheck, NoteKind, Notes, Overwrite};
    use proptest::prelude::*;

    struct Fix {
        _dir: TmpDir,
        host: Arc<TestHost>,
        store: Arc<DocStore>,
        mgr: SessionManager,
        files: std::path::PathBuf,
    }

    impl Fix {
        fn new() -> Self {
            let dir = TmpDir::new("outside3");
            let host = Arc::new(TestHost {
                now: AtomicU64::new(1_000_000),
                rights: Mutex::new(HashMap::new()),
                events: Mutex::new(Vec::new()),
            });
            for p in ["p1", "p2", "bot"] {
                host.rights
                    .lock()
                    .unwrap()
                    .insert(p.into(), Rights::full().with_owner(true));
            }
            let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).unwrap());
            let mgr = SessionManager::new(store.clone(), host.clone(), config());
            let files = dir.0.join("files");
            std::fs::create_dir_all(&files).unwrap();
            Fix {
                _dir: dir,
                host,
                store,
                mgr,
                files,
            }
        }
        fn file(&self) -> std::path::PathBuf {
            self.files.join("t.csv")
        }
        fn open(&self) -> Arc<Session> {
            self.mgr
                .open_file(Arc::new(Lite), &key("t.csv"), &self.file(), &self.files)
                .unwrap()
        }
        fn join(&self, s: &Session, pid: &str) -> ConnId {
            s.join(participant(pid, ParticipantKind::Person), Stay::Held)
                .unwrap()
                .conn
        }
        fn advance(&self, ms: u64) {
            self.host.now.fetch_add(ms, Ordering::SeqCst);
        }
        fn restart(&mut self) {
            self.mgr = SessionManager::new(self.store.clone(), self.host.clone(), config());
        }
        fn bytes(&self) -> String {
            std::fs::read_to_string(self.file()).unwrap()
        }
        /// Another program saves the file; the watcher reports it.
        fn save(&self, s: &Session, text: &str) -> DiskCheck {
            self.advance(500);
            std::fs::write(self.file(), text).unwrap();
            s.file_changed().unwrap()
        }
        fn overrides(&self) -> Vec<(String, String, Option<String>)> {
            self.host
                .events
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    AuditEvent::OutsideOverride {
                        rule, region, by, ..
                    } => Some(((*rule).to_owned(), region.clone(), by.clone())),
                    _ => None,
                })
                .collect()
        }
    }

    fn table_of(s: &Session, c: ConnId) -> Table {
        View::of_content(&s.read(c).unwrap().content)
            .table()
            .unwrap()
    }

    fn render(t: &Table) -> String {
        t.render()
            .iter()
            .map(|r| {
                let f: Vec<&str> = r.iter().map(|f| f.value.as_str()).collect();
                format!("{}\n", f.join(","))
            })
            .collect()
    }

    fn set(s: &Session, c: ConnId, r: &str, col: &str, v: &str) {
        op(s, c, &EditOptions::new(), |t| t.set_cell(&k(r), &k(col), v)).unwrap();
    }

    /// Each cell a note speaks for: `(region, rule, yours, file, version)`.
    fn cells_of(ow: &[Overwrite]) -> Vec<(String, &'static str, String, String, Option<u64>)> {
        let mut out = Vec::new();
        for o in ow {
            out.push((
                o.region.clone(),
                o.rule,
                o.yours.clone(),
                o.file.clone(),
                o.version,
            ));
            for c in &o.also {
                out.push((
                    c.region.clone(),
                    o.rule,
                    c.yours.clone(),
                    c.file.clone(),
                    o.version,
                ));
            }
        }
        out
    }

    fn overwritten(n: &Notes) -> Vec<Overwrite> {
        n.notes
            .iter()
            .filter(|x| x.kind == NoteKind::Overwritten)
            .map(|x| x.overwrite.clone().expect("an Overwritten note says what"))
            .collect()
    }

    /// The version holding `v` in `cell`, by the note's id.
    fn version_holds(s: &Session, c: ConnId, vid: u64, r: &str, col: &str) -> Option<String> {
        let content = s.show_content(c, vid).unwrap();
        View::of_content(&content)
            .table()
            .unwrap()
            .cell(&k(r), &k(col))
            .map(str::to_owned)
    }

    /// v0.9.5's echo rule on a grid (as for text): a file holding one of the kit's recent
    /// writes is its echo (`OwnWrite`) only until a person's save comes in; after that, a file
    /// back at an earlier write of the kit's is that person's edit (their editor's undo):
    /// ingested, with its content, a new version and an External change.
    #[test]
    fn after_a_persons_save_an_earlier_kit_write_is_their_edit() {
        use k2_mpc_base::session::{ChangeKind, LogItem, LogQuery, EXTERNAL_PID};
        let f = Fix::new();
        std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        // two writes of the kit's
        set(&s, p1, "r1", "c1", "A");
        f.advance(2_000);
        assert!(s.flush(p1).unwrap().written);
        let first = f.bytes();
        assert_eq!(first, "A,b\nc,d\n");
        set(&s, p1, "r2", "c2", "D");
        f.advance(2_000);
        assert!(s.flush(p1).unwrap().written);
        assert_eq!(f.bytes(), "A,b\nc,D\n");
        // the first coming back now is the kit's echo: not an edit
        assert_eq!(f.save(&s, &first), DiskCheck::OwnWrite);
        f.advance(2_000);
        assert!(s.flush(p1).unwrap().written, "the doc is written back");
        // a person's save comes in
        assert!(matches!(
            f.save(&s, "A,b\nc,X\n"),
            DiskCheck::Ingested { .. }
        ));
        let v1 = s.status().unwrap().version;
        // the file back at the kit's first write: the person's edit now
        let check = f.save(&s, &first);
        assert!(matches!(check, DiskCheck::Ingested { .. }), "{check:?}");
        assert_eq!(text(&s, p1), first, "the doc is the file");
        let v2 = s.status().unwrap().version;
        assert!(v2 > v1, "a new version: {v1} -> {v2}");
        let log = s.log(p1, &LogQuery::default()).unwrap();
        let newest = log
            .iter()
            .find_map(|i| match i {
                LogItem::Change { info, .. } => Some(info.clone()),
                _ => None,
            })
            .expect("a change");
        assert_eq!(
            (newest.seq, newest.kind, newest.author.as_str()),
            (v2, ChangeKind::External, EXTERNAL_PID)
        );
    }

    /// Same cell on both sides: the file's value wins; the live value is in the version
    /// cut before the save, named in a note to its author with both values and the address,
    /// and in an audit event; the author's undo then conflicts instead of clobbering.
    #[test]
    fn same_cell_the_file_wins_and_the_live_value_is_kept() {
        let f = Fix::new();
        std::fs::write(f.file(), "a,b\nc,d\ne,f\n").unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        set(&s, p1, "r2", "c2", "LIVE");
        // within the write delay: the file never got it
        assert_eq!(f.bytes(), "a,b\nc,d\ne,f\n");
        let check = f.save(&s, "a,b\nc,FILE\ne,f\n");
        assert!(matches!(check, DiskCheck::Ingested { .. }), "{check:?}");
        let t = table_of(&s, p1);
        assert_eq!(t.cell(&k("r2"), &k("c2")), Some("FILE"), "the file wins");
        let b = s.blame_cell(p1, &k("r2"), &k("c2")).unwrap().unwrap();
        assert_eq!(b.author, k2_mpc_base::session::EXTERNAL_PID);
        // the note to the live value's author
        let n = s.take_notes(p1).unwrap();
        let ow = overwritten(&n);
        assert_eq!(ow.len(), 1, "{n:?}");
        let o = &ow[0];
        assert_eq!((o.rule, o.region.as_str()), ("cell", "r2/c2"));
        assert_eq!((o.line, o.col), (Some(2), Some(2)));
        assert_eq!(
            (o.yours.as_str(), o.file.as_str()),
            ("\"LIVE\"", "\"FILE\"")
        );
        let line = n.notes[0].to_string();
        assert!(
            line.starts_with("note: an outside save overwrote r2/c2 (L2, col 2): yours \"LIVE\""),
            "{line}"
        );
        // the version cut before the save holds the live value
        let vid = o.version.expect("a version holds the live value");
        assert_eq!(
            version_holds(&s, p1, vid, "r2", "c2").as_deref(),
            Some("LIVE")
        );
        assert!(s
            .versions(p1)
            .unwrap()
            .iter()
            .any(|v| v.id == vid && v.reason == VersionReason::BeforeExternal));
        // the audit event, without content
        assert_eq!(
            f.overrides(),
            [("cell".into(), "r2/c2".into(), Some("p1".into()))]
        );
        // drained; and p1's undo conflicts (the cell was written by someone else since)
        assert!(s.take_notes(p1).unwrap().notes.is_empty());
        let u = s
            .undo_with(p1, &UndoRequest::steps(1).mode(UndoMode::All))
            .unwrap();
        assert!(matches!(u, UndoOutcome::Conflicts { .. }), "{u:?}");
        assert_eq!(table_of(&s, p1).cell(&k("r2"), &k("c2")), Some("FILE"));
        // the file and the doc agree after the write-back
        f.advance(2_000);
        s.flush(p1).unwrap();
        assert_eq!(f.bytes(), "a,b\nc,FILE\ne,f\n");
        assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
    }

    /// The overwritten live value is **recoverable**, not only stored: after the same save
    /// (LIVE set in the write delay, then FILE saved in the cell), (1) a version restore to
    /// the BeforeExternal version the note names brings `r2/c2 = LIVE` back, the other cells
    /// as they were, the audit clean; (2) a revert of the External change brings LIVE back,
    /// and blame names the original LIVE write.
    #[test]
    fn the_overwritten_live_value_is_recoverable() {
        use k2_mpc_base::session::{ChangeKind, LogItem, LogQuery};
        let setup = || {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\ne,f\n").unwrap();
            let s = f.open();
            let p1 = f.join(&s, "p1");
            let live = op(&s, p1, &EditOptions::new(), |t| {
                t.set_cell(&k("r2"), &k("c2"), "LIVE")
            })
            .unwrap();
            let check = f.save(&s, "a,b\nc,FILE\ne,f\n");
            assert!(matches!(check, DiskCheck::Ingested { .. }), "{check:?}");
            assert_eq!(table_of(&s, p1).cell(&k("r2"), &k("c2")), Some("FILE"));
            let ow = overwritten(&s.take_notes(p1).unwrap());
            let vid = ow[0].version.expect("a version holds the live value");
            (f, s, p1, live, vid)
        };
        let cells = |s: &Session, c: ConnId| {
            let t = table_of(s, c);
            t.live_rows()
                .iter()
                .map(|r| {
                    t.live_cols()
                        .iter()
                        .map(|col| t.cell(r, col).unwrap_or("").to_owned())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let want = vec![
            vec!["a".to_owned(), "b".to_owned()],
            vec!["c".to_owned(), "LIVE".to_owned()],
            vec!["e".to_owned(), "f".to_owned()],
        ];

        // (1) a version restore to the BeforeExternal version
        let (f, s, p1, _, vid) = setup();
        assert!(s
            .versions(p1)
            .unwrap()
            .iter()
            .any(|v| v.id == vid && v.reason == VersionReason::BeforeExternal));
        s.restore(p1, vid).unwrap();
        assert_eq!(
            cells(&s, p1),
            want,
            "restored: LIVE back, the rest as it was"
        );
        f.advance(2_000);
        s.flush(p1).unwrap();
        assert_eq!(f.bytes(), "a,b\nc,LIVE\ne,f\n");
        assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
        assert_eq!(s.audit_disk_sync().unwrap(), vec![]);

        // (2) a revert of the External change
        let (f, s, p1, live, _) = setup();
        let ext = s
            .log(p1, &LogQuery::default())
            .unwrap()
            .into_iter()
            .find_map(|i| match i {
                LogItem::Change { info, .. } if info.kind == ChangeKind::External => Some(info.seq),
                _ => None,
            })
            .expect("the outside save's change");
        let r = s.revert(p1, ext, false).unwrap();
        assert!(matches!(r, UndoOutcome::Applied { .. }), "{r:?}");
        assert_eq!(
            cells(&s, p1),
            want,
            "reverted: LIVE back, the rest as it was"
        );
        let b = s.blame_cell(p1, &k("r2"), &k("c2")).unwrap().unwrap();
        assert_eq!(
            (b.author.as_str(), b.change),
            ("p1", live),
            "blame names the original LIVE write: {b:?}"
        );
        f.advance(2_000);
        s.flush(p1).unwrap();
        assert_eq!(f.bytes(), "a,b\nc,LIVE\ne,f\n");
        assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
    }

    /// An outside save's notes belong to the change it just made: `AuditEvent::NotesWithoutChange`
    /// stays quiet.
    #[test]
    fn an_outside_save_with_a_note_reports_no_notes_without_a_change() {
        let f = Fix::new();
        std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        set(&s, p1, "r2", "c2", "LIVE");
        f.save(&s, "a,b\nc,FILE\n");
        assert_eq!(overwritten(&s.take_notes(p1).unwrap()).len(), 1, "the note");
        let events = f.host.events.lock().unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AuditEvent::NotesWithoutChange { .. })),
            "{events:?}"
        );
    }

    /// A row deleted in the file, edited live: the delete wins; the row stays restorable with
    /// the live value (P43), and the editor's note names it.
    #[test]
    fn a_row_the_file_deletes_and_live_edits_is_deleted_and_restorable() {
        let f = Fix::new();
        std::fs::write(f.file(), "a,b\nc,d\ne,f\n").unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        set(&s, p1, "r2", "c1", "LIVE");
        f.save(&s, "a,b\ne,f\n");
        let t = table_of(&s, p1);
        assert_eq!(t.live_rows(), [k("r1"), k("r3")], "the file's delete wins");
        let ow = overwritten(&s.take_notes(p1).unwrap());
        assert_eq!(ow.len(), 1, "{ow:?}");
        assert_eq!(
            (ow[0].rule, ow[0].region.as_str()),
            ("edited_row", "row r2")
        );
        assert_eq!(ow[0].line, Some(2));
        let vid = ow[0].version.unwrap();
        assert_eq!(
            version_holds(&s, p1, vid, "r2", "c1").as_deref(),
            Some("LIVE")
        );
        assert_eq!(f.overrides()[0].0, "edited_row");
        // restorable, with the live value
        op(&s, p1, &EditOptions::new(), |t| t.restore_row(&k("r2"))).unwrap();
        assert_eq!(table_of(&s, p1).cell(&k("r2"), &k("c1")), Some("LIVE"));
    }

    /// A row deleted live, edited in the file: the file's edit wins and the row comes back
    /// (restored, then set), with a note to whoever deleted it.
    #[test]
    fn a_row_live_deletes_and_the_file_edits_comes_back() {
        let f = Fix::new();
        std::fs::write(f.file(), "a,b\nc,d\ne,f\n").unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        let p2 = f.join(&s, "p2");
        op(&s, p2, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
        f.save(&s, "a,b\nc,FILE\ne,f\n");
        let t = table_of(&s, p1);
        assert_eq!(
            t.live_rows(),
            [k("r1"), k("r2"), k("r3")],
            "back, in its place"
        );
        assert_eq!(t.cell(&k("r2"), &k("c2")), Some("FILE"));
        assert_eq!(
            t.cell(&k("r2"), &k("c1")),
            Some("c"),
            "its other cells with it"
        );
        let ow = overwritten(&s.take_notes(p2).unwrap());
        assert_eq!(ow.len(), 1, "{ow:?}");
        assert_eq!(
            (ow[0].rule, ow[0].region.as_str()),
            ("deleted_row", "r2/c2")
        );
        assert!(
            ow[0].yours.contains("deleted") || ow[0].yours == "\"d\"",
            "{:?}",
            ow[0]
        );
        assert!(s.take_notes(p1).unwrap().notes.is_empty(), "p1 did nothing");
        // the version before the save holds the row deleted
        let vid = ow[0].version.unwrap();
        let v = View::of_content(&s.show_content(p1, vid).unwrap())
            .table()
            .unwrap();
        assert!(!v.rows.is_live(&k("r2")));
        assert_eq!(f.overrides()[0].2.as_deref(), Some("p2"));
    }

    /// The column forms: a column the file deletes and live edits is deleted (and
    /// restorable); a column live deletes and the file edits comes back.
    #[test]
    fn columns_as_rows() {
        let f = Fix::new();
        std::fs::write(f.file(), "a,b,x\nc,d,y\ne,f,z\n").unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        set(&s, p1, "r1", "c2", "LIVE");
        f.save(&s, "a,x\nc,y\ne,z\n");
        let t = table_of(&s, p1);
        assert_eq!(t.live_cols(), [k("c1"), k("c3")]);
        let ow = overwritten(&s.take_notes(p1).unwrap());
        assert_eq!(
            (ow[0].rule, ow[0].region.as_str()),
            ("edited_col", "col c2")
        );
        assert_eq!(ow[0].col, Some(2));
        op(&s, p1, &EditOptions::new(), |t| t.restore_col(&k("c2"))).unwrap();
        assert_eq!(table_of(&s, p1).cell(&k("r1"), &k("c2")), Some("LIVE"));
        // live deletes c3, the file edits a cell in it: c3 comes back with the file's value
        f.advance(2_000);
        s.flush(p1).unwrap();
        let p2 = f.join(&s, "p2");
        op(&s, p2, &EditOptions::new(), |t| t.delete_col(&k("c3"))).unwrap();
        let synced = f.bytes();
        f.save(&s, &synced.replacen("y", "FILE", 1));
        let t = table_of(&s, p1);
        assert!(t.cols.is_live(&k("c3")));
        assert_eq!(t.cell(&k("r2"), &k("c3")), Some("FILE"));
        let ow = overwritten(&s.take_notes(p2).unwrap());
        assert_eq!(ow[0].rule, "deleted_col");
        assert!(f
            .overrides()
            .iter()
            .any(|o| o.0 == "deleted_col" && o.2.as_deref() == Some("p2")));
    }

    /// Both moved one row: the file's place, with no note.
    #[test]
    fn a_row_moved_both_ways_goes_to_the_files_place() {
        let f = Fix::new();
        std::fs::write(f.file(), "a\nb\nc\nd\n").unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        op(&s, p1, &EditOptions::new(), |t| {
            t.move_row(&k("r4"), Anchor::Start)
        })
        .unwrap();
        f.save(&s, "a\nd\nb\nc\n");
        let t = table_of(&s, p1);
        assert_eq!(t.live_rows(), [k("r1"), k("r4"), k("r2"), k("r3")]);
        // a move is no change: the file's place, no note (K2 Cores' ruling (d))
        assert!(overwritten(&s.take_notes(p1).unwrap()).is_empty());
    }

    /// Changes on other elements both apply, with no note.
    #[test]
    fn disjoint_changes_both_apply_with_no_note() {
        let f = Fix::new();
        std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        set(&s, p1, "r1", "c1", "LIVE");
        op(&s, p1, &EditOptions::new(), |t| {
            t.insert_row(Anchor::End, &["x".into(), "y".into()])
                .map(|x| x.1)
        })
        .unwrap();
        f.save(&s, "a,b\nc,FILE\nnew,row\n");
        let t = table_of(&s, p1);
        // rows both sides inserted at one gap: the file's land right after their left
        // neighbour, as the aligner anchors a file's rows (before the row live appended)
        assert_eq!(render(&t), "LIVE,b\nc,FILE\nnew,row\nx,y\n");
        assert!(s.take_notes(p1).unwrap().notes.is_empty());
        assert!(f.overrides().is_empty());
        f.advance(2_000);
        s.flush(p1).unwrap();
        assert_eq!(f.bytes(), "LIVE,b\nc,FILE\nnew,row\nx,y\n");
        assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
        assert!(s.audit().unwrap().is_clean());
    }

    /// The other order: an unwritten edit, then a header switch both ways (rows re-keyed,
    /// the bytes as they were), then an outside save of the switched line.
    #[test]
    fn an_unwritten_edit_then_a_header_switch_then_a_save() {
        let f = Fix::new();
        std::fs::write(f.file(), "a,b,c\nd,e,f\ng,h,i\n").unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        set(&s, p1, "r3", "c3", "LIVE");
        op(&s, p1, &EditOptions::new(), |t| t.set_header(true)).unwrap();
        op(&s, p1, &EditOptions::new(), |t| t.set_header(false)).unwrap();
        f.save(&s, "a,FILE,c\nd,e,f\ng,h,i\n");
        let t = table_of(&s, p1);
        assert_eq!(render(&t), "a,FILE,c\nd,e,f\ng,h,LIVE\n");
        f.advance(2_000);
        s.flush(p1).unwrap();
        assert_eq!(f.bytes(), "a,FILE,c\nd,e,f\ng,h,LIVE\n");
        assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
    }

    /// The same, with the unwritten edit on the switched row itself: it is matched by place.
    #[test]
    fn an_edit_on_the_switched_row_then_a_header_switch_then_a_save() {
        let f = Fix::new();
        std::fs::write(f.file(), "a,b,c\nd,e,f\ng,h,i\n").unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        set(&s, p1, "r1", "c3", "LIVE");
        op(&s, p1, &EditOptions::new(), |t| t.set_header(true)).unwrap();
        op(&s, p1, &EditOptions::new(), |t| t.set_header(false)).unwrap();
        f.save(&s, "a,FILE,c\nd,e,f\ng,h,i\n");
        let t = table_of(&s, p1);
        assert_eq!(
            render(&t),
            "a,FILE,c\nd,e,f\ng,h,i\n",
            "a row both changed: the file's"
        );
        let ow = overwritten(&s.take_notes(p1).unwrap());
        assert_eq!(ow.len(), 1, "{ow:?}");
        assert_eq!(
            (
                ow[0].region.as_str(),
                ow[0].yours.as_str(),
                ow[0].file.as_str()
            ),
            // (the header switch re-identified the row: its key now is r4)
            ("r4/c3", "\"LIVE\"", "\"c\"")
        );
        let vid = ow[0].version.expect("a version holds the live value");
        assert_eq!(
            version_holds(&s, p1, vid, "r4", "c3").as_deref(),
            Some("LIVE")
        );
        f.advance(2_000);
        s.flush(p1).unwrap();
        assert_eq!(f.bytes(), "a,FILE,c\nd,e,f\ng,h,i\n");
        assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
        // one switch (off -> on) with an edit pending, then a save of the header line
        set(&s, p1, "r2", "c1", "LIVE2");
        op(&s, p1, &EditOptions::new(), |t| t.set_header(true)).unwrap();
        f.save(&s, "a,FILE,NAME\nd,e,f\ng,h,i\n");
        let t = table_of(&s, p1);
        assert!(t.header);
        assert_eq!(render(&t), "a,FILE,NAME\nLIVE2,e,f\ng,h,i\n");
    }

    fn soak_rounds() -> u64 {
        std::env::var("GRID_SOAK_ROUNDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(150)
    }

    /// A soak-like long run with outside saves (there is no grid soak yet): a person and an
    /// agent edit cells, insert, move, delete and restore rows, switch the header; write-backs,
    /// ticks and restarts come and go; a plain editor saves the file (cells rewritten, a
    /// line appended or removed), inside the write delay too. After every save each value it
    /// wrote is in the live grid (the file wins, never dropped), and after every write-back
    /// the file is the doc, K-AUD-1's sync check and the lost-edit audit are clean.
    #[test]
    fn grid_soak_with_outside_saves() {
        let rounds = soak_rounds();
        let mut rng: u64 = std::env::var("GRID_SOAK_SEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0x9e37_79b9_7f4a_7c15);
        let mut next = move |n: u64| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng % n.max(1)
        };
        let mut f = Fix::new();
        std::fs::write(f.file(), "a0,b0,c0\na1,b1,c1\na2,b2,c2\na3,b3,c3\n").unwrap();
        let mut s = f.open();
        let mut p1 = f.join(&s, "p1");
        let mut bot = s
            .join(participant("bot", ParticipantKind::Agent), Stay::Held)
            .unwrap()
            .conn;
        let (mut saves, mut overrides) = (0u64, 0usize);
        // a person typing with a view one round old (a seen version): how often a text's marks
        // bounce an edit, and it succeeds once the view is current
        let (mut typed, mut bounces, mut remap_bounces) = (0u64, 0u64, 0u64);
        let mut seen = s.status().unwrap().version;
        for round in 0..rounds {
            let t = table_of(&s, p1);
            let rows = t.live_rows();
            let cols = t.live_cols();
            let pick = |v: &[Key], n: u64| v[(n as usize) % v.len()].clone();
            let o = EditOptions::new();
            let ov = EditOptions::new().if_version(s.status().unwrap().version);
            match next(10) {
                0..=2 if !rows.is_empty() => {
                    let (r, c) = (pick(&rows, next(97)), pick(&cols, next(97)));
                    let v = format!("p{round}");
                    op(&s, p1, &o, |t| t.set_cell(&r, &c, &v)).unwrap();
                }
                3 => {
                    let at = if rows.is_empty() || next(2) == 0 {
                        Anchor::End
                    } else {
                        Anchor::After(pick(&rows, next(97)))
                    };
                    let vals: Vec<String> =
                        (0..cols.len()).map(|j| format!("i{round}x{j}")).collect();
                    op(&s, bot, &ov, |t| t.insert_row(at, &vals).map(|x| x.1)).unwrap();
                }
                4 if rows.len() > 2 => {
                    let r = pick(&rows, next(97));
                    op(&s, bot, &ov, |t| t.delete_row(&r)).unwrap();
                }
                5 if rows.len() > 1 => {
                    let r = pick(&rows, next(97));
                    op(&s, bot, &ov, |t| t.move_row(&r, Anchor::Start)).unwrap();
                }
                6 => {
                    let gone: Vec<Key> = t.rows.deleted().cloned().collect();
                    if !gone.is_empty() {
                        let r = pick(&gone, next(97));
                        op(&s, bot, &ov, |t| t.restore_row(&r)).unwrap();
                    }
                }
                7 if next(4) == 0 && rows.len() > 1 => {
                    let on = !t.header;
                    op(&s, p1, &o, |t| t.set_header(on)).unwrap();
                }
                8 => {
                    f.advance(if next(2) == 0 { 300 } else { 2_000 });
                    let r = f.mgr.tick().unwrap();
                    assert!(r.errors.is_empty(), "round {round}: {:?}", r.errors);
                }
                _ => {
                    if next(3) == 0 {
                        drop(s);
                        f.restart();
                        s = f.open();
                        p1 = f.join(&s, "p1");
                        bot = s
                            .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                            .unwrap()
                            .conn;
                    } else {
                        s.flush(p1).unwrap();
                    }
                }
            }
            {
                let t = table_of(&s, p1);
                let (rows, cols) = (t.live_rows(), t.live_cols());
                if !rows.is_empty() && !cols.is_empty() && next(2) == 0 {
                    let (r, c) = (
                        rows[next(97) as usize % rows.len()].clone(),
                        cols[next(97) as usize % cols.len()].clone(),
                    );
                    let v = format!("t{round}");
                    typed += 1;
                    let o = EditOptions::new().seen(seen);
                    match op(&s, p1, &o, |t| t.set_cell(&r, &c, &v)) {
                        Ok(_) => {}
                        Err(SessionError::FacetConflict(c0))
                            if c0.iter().all(|x| x.rewritten_row) =>
                        {
                            bounces += 1;
                            remap_bounces += u64::from(c0.iter().any(|x| x.root == table::COLS));
                            let now = EditOptions::new().seen(s.status().unwrap().version);
                            op(&s, p1, &now, |t| t.set_cell(&r, &c, &v)).unwrap();
                        }
                        Err(e) => panic!("round {round}: {e}"),
                    }
                }
                seen = s.status().unwrap().version;
            }
            // a plain editor saves the file, often
            if next(3) == 0 {
                let text = f.bytes();
                let mut lines: Vec<Vec<String>> = text
                    .lines()
                    .map(|l| l.split(',').map(str::to_owned).collect())
                    .collect();
                let mut wrote = Vec::new();
                let nl = lines.len().max(1) as u64;
                for n in 0..=next(2) {
                    let i = next(nl) as usize;
                    if let Some(line) = lines.get_mut(i) {
                        if line.is_empty() || line == &vec![String::new()] {
                            continue;
                        }
                        let j = next(line.len() as u64) as usize;
                        // a field this save already wrote keeps that value
                        if line[j].starts_with(&format!("S{round}x")) {
                            continue;
                        }
                        let v = format!("S{round}x{n}");
                        line[j] = v.clone();
                        wrote.push(v);
                    }
                }
                match next(6) {
                    0 => {
                        let w = lines.first().map_or(3, Vec::len);
                        let v = format!("A{round}");
                        lines.push(vec![v.clone(); w]);
                        wrote.push(v);
                    }
                    1 if lines.len() > 3 => {
                        let i = 1 + next(lines.len() as u64 - 1) as usize;
                        let gone = lines.remove(i);
                        wrote.retain(|v| !gone.contains(v));
                    }
                    // a column swap, now and then: a remap, the widest bounce there is
                    2 if next(4) == 0 => {
                        for l in lines.iter_mut().filter(|l| l.len() > 1) {
                            l.swap(0, 1);
                        }
                    }
                    _ => {}
                }
                let saved: String = lines.iter().map(|l| format!("{}\n", l.join(","))).collect();
                if saved != text {
                    f.save(&s, &saved);
                    saves += 1;
                    let live = String::from_utf8(s.read(p1).unwrap().bytes).unwrap();
                    for v in &wrote {
                        assert!(
                            live.contains(v.as_str()),
                            "round {round}: the save's {v} was dropped:\nsaved:\n{saved}live:\n{live}"
                        );
                    }
                    overrides += s.take_notes(p1).unwrap().notes.len();
                }
            }
            if round % 10 == 9 {
                f.advance(2_000);
                s.flush(p1).unwrap();
                let live = String::from_utf8(s.read(p1).unwrap().bytes).unwrap();
                assert_eq!(f.bytes(), live, "round {round}: the file is the doc");
                assert_eq!(
                    s.audit_disk_sync().unwrap(),
                    vec![],
                    "round {round}: K-AUD-1"
                );
                let a = s.audit().unwrap();
                assert!(a.is_clean(), "round {round}: {a:?}");
            }
        }
        println!(
            "RUNREC {{\"job\":null,\"run\":\"grid_soak_outside\",\"rounds\":{rounds},\"saves\":{saves},\"overrides_noted\":{overrides},\"typed\":{typed},\"bounces\":{bounces},\"remap_bounces\":{remap_bounces},\"status\":\"PASSED\"}}"
        );
    }

    /// The byte-neutral change before the unwritten edits and the save: the no-byte twin's
    /// kinds (`no_byte::Change`, but its header switch on, which renumbers the data lines),
    /// so each is followed by an unwritten edit before an outside save here.
    #[derive(Debug, Clone, Copy)]
    enum Neutral {
        None,
        HeaderOnOff,
        UnsetEmpty,
        DropDeleted,
        CellSetBack,
        CellSetUndo,
        RowDeleteUndo,
        RowMoveUndo,
        Revert,
        Restore,
    }

    #[derive(Debug, Clone, Copy)]
    enum Between {
        Nothing,
        Flush,
        Restart,
    }

    fn applied(r: UndoOutcome, why: &str) {
        assert!(
            matches!(r, UndoOutcome::Applied { ref skipped, .. } if skipped.is_empty()),
            "{why}: {r:?}"
        );
    }

    fn cases() -> u32 {
        std::env::var("GRID_OUTSIDE_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(128)
    }

    /// One case: a byte-neutral change (or none), then a write-back or a restart (or
    /// nothing), then unwritten live cell edits, then an outside save changing cells (some
    /// the live side changed too) and maybe appending a row. Checked against the model: the
    /// live grid is the live table with the file's changes by key (the file wins on a cell
    /// both changed), keys kept, blame (the file's cells to `kit:external`, the live-only
    /// cells to their writer), one note and one audit event per overridden cell, the version
    /// before the save holding every overridden live value, and the bytes after the
    /// write-back.
    #[allow(clippy::too_many_arguments)]
    fn case(
        (nr0, nc): (usize, usize),
        neutral: Neutral,
        between: Between,
        live: &[(usize, usize, usize)],
        outside: &[(usize, usize, usize)],
        append: bool,
    ) {
        let why = format!(
            "{neutral:?}, {between:?}, live {live:?}, outside {outside:?}, append {append}"
        );
        // unique values, so the aligner matches every row the save edits one cell of
        let mut seed: Vec<Vec<String>> = (0..nr0)
            .map(|i| (0..nc).map(|j| format!("v{i}x{j}")).collect())
            .collect();
        if matches!(neutral, Neutral::UnsetEmpty) {
            seed[0][0] = String::new();
        }
        let text0: String = seed.iter().map(|r| format!("{}\n", r.join(","))).collect();
        let mut f = Fix::new();
        std::fs::write(f.file(), &text0).unwrap();
        let mut s = f.open();
        let mut p1 = f.join(&s, "p1");
        let o = EditOptions::new();
        match neutral {
            Neutral::None => {}
            Neutral::HeaderOnOff => {
                op(&s, p1, &o, |t| t.set_header(true)).unwrap();
                op(&s, p1, &o, |t| t.set_header(false)).unwrap();
            }
            Neutral::UnsetEmpty => {
                s.edit_ops(
                    p1,
                    vec![Edit::Map {
                        root: CELLS.into(),
                        ops: vec![MapOp::Delete {
                            key: table::cell_key(&k("r1"), &k("c1")),
                        }],
                    }],
                    &o,
                )
                .unwrap();
            }
            Neutral::DropDeleted => {
                let last = k(&format!("r{}", seed.len()));
                op(&s, p1, &o, |t| t.delete_row(&last)).unwrap();
                assert!(s.flush(p1).unwrap().written, "{why}");
                seed.pop();
                s.drop_deleted(p1).unwrap();
            }
            Neutral::CellSetBack => {
                op(&s, p1, &o, |t| t.set_cell(&k("r1"), &k("c1"), "tmp")).unwrap();
                let back = seed[0][0].clone();
                op(&s, p1, &o, |t| t.set_cell(&k("r1"), &k("c1"), &back)).unwrap();
            }
            Neutral::CellSetUndo => {
                op(&s, p1, &o, |t| t.set_cell(&k("r1"), &k("c1"), "tmp")).unwrap();
                applied(s.undo(p1, 1).unwrap(), &why);
            }
            Neutral::RowDeleteUndo => {
                op(&s, p1, &o, |t| t.delete_row(&k("r1"))).unwrap();
                applied(s.undo(p1, 1).unwrap(), &why);
            }
            Neutral::RowMoveUndo => {
                let last = k(&format!("r{}", seed.len()));
                op(&s, p1, &o, |t| t.move_row(&last, Anchor::Start)).unwrap();
                applied(s.undo(p1, 1).unwrap(), &why);
            }
            Neutral::Revert => {
                let v = op(&s, p1, &o, |t| t.set_cell(&k("r1"), &k("c1"), "tmp")).unwrap();
                applied(s.revert(p1, v, false).unwrap(), &why);
            }
            Neutral::Restore => {
                let v = s.name_version(p1, "before", None).unwrap();
                op(&s, p1, &o, |t| t.set_cell(&k("r1"), &k("c1"), "tmp")).unwrap();
                s.restore(p1, v).unwrap();
            }
        }
        let synced: String = seed.iter().map(|r| format!("{}\n", r.join(","))).collect();
        assert_eq!(text(&s, p1), synced, "{why}: the change moved no byte");
        match between {
            Between::Nothing => {}
            Between::Flush => {
                assert!(!s.flush(p1).unwrap().written, "{why}: nothing to write");
            }
            Between::Restart => {
                drop(s);
                f.restart();
                s = f.open();
                p1 = f.join(&s, "p1");
            }
        }
        assert_eq!(
            s.audit_disk_sync().unwrap(),
            vec![],
            "{why}: K-AUD-1 before"
        );
        let nr = seed.len();
        // the line -> row key map as the file holds it (live renders the same bytes)
        let t0 = table_of(&s, p1);
        let lm = t0.line_map();
        let row_at = |i: usize| lm.row_at(i % nr + 1).expect("a data line").clone();
        let col_at = |j: usize| t0.live_cols()[j % nc].clone();
        // unwritten live edits
        let mut live_set: HashMap<(Key, Key), String> = HashMap::new();
        for (n, (i, j, v)) in live.iter().enumerate() {
            let (r, c) = (row_at(*i), col_at(*j));
            let val = format!("L{n}v{v}");
            op(&s, p1, &o, |t| t.set_cell(&r, &c, &val)).unwrap();
            live_set.insert((r, c), val);
        }
        let before = table_of(&s, p1);
        let blame_before: HashMap<(Key, Key), String> = before
            .live_rows()
            .iter()
            .flat_map(|r| before.live_cols().into_iter().map(move |c| (r.clone(), c)))
            .filter_map(|(r, c)| {
                let b = s.blame_cell(p1, &r, &c).unwrap()?;
                Some(((r, c), b.author))
            })
            .collect();
        // the outside save: from the synced bytes
        let mut rows: Vec<Vec<String>> = seed.clone();
        let mut file_set: HashMap<(Key, Key), String> = HashMap::new();
        let mut edited_rows = Vec::new();
        for (n, (i, j, v)) in outside.iter().enumerate() {
            let (ri, cj) = (i % nr, j % nc);
            // one cell per row: the row stays recognisable
            if edited_rows.contains(&ri) {
                continue;
            }
            edited_rows.push(ri);
            let val = format!("F{n}v{v}");
            rows[ri][cj] = val.clone();
            file_set.insert((row_at(ri), col_at(cj)), val);
        }
        if append {
            rows.push(vec!["new".to_owned(); nc]);
        }
        let saved: String = rows.iter().map(|r| format!("{}\n", r.join(","))).collect();
        // the model: the live table, then for each row the file changed, the file's row whole
        // (contract §2.9.1): every live cell on it that differs from the file's is overridden.
        // A live cell equal to the file's overwrites nothing, so it has no note
        let mut want = before.clone();
        let mut overridden: Vec<(Key, Key, String)> = Vec::new();
        let mut held: HashMap<(Key, Key), Option<String>> = HashMap::new();
        for ((r, c), v) in &file_set {
            if v != t0.cell(r, c).unwrap_or("") {
                want.set_cell(r, c, v).unwrap();
                held.insert((r.clone(), c.clone()), Some(v.clone()));
            }
        }
        let file_rows: Vec<Key> = held.keys().map(|(r, _)| r.clone()).collect();
        // the rows both sides changed: the file's row is written whole, so each of its cells is
        // the file's field. `live_set` holds real live edits only (the byte-neutral change is not
        // in it), so a row whose only live change is byte-neutral is not here. A non-empty field
        // compares exactly; an empty one as `""`, since the file's parse can't tell an empty
        // field from an absent cell (an unchanged empty field keeps whichever the base has)
        let whole_rows: Vec<Key> = file_rows
            .iter()
            .filter(|r| live_set.keys().any(|(x, _)| x == *r))
            .cloned()
            .collect();
        let file_field = |r: &Key, c: &Key| -> Option<String> {
            let ri = (0..nr).find(|i| row_at(*i) == *r)?;
            let j = t0.live_cols().iter().position(|x| x == c)?;
            rows[ri].get(j).cloned()
        };
        for ((r, c), lv) in &live_set {
            if !file_rows.contains(r) {
                continue;
            }
            let fv = file_set
                .get(&(r.clone(), c.clone()))
                .map(String::as_str)
                .or_else(|| t0.cell(r, c));
            if fv != Some(lv.as_str()) {
                overridden.push((r.clone(), c.clone(), lv.clone()));
                held.insert((r.clone(), c.clone()), fv.map(str::to_owned));
            }
        }
        let check = f.save(&s, &saved);
        if saved != synced {
            assert!(
                matches!(check, DiskCheck::Ingested { .. }),
                "{why}: {check:?}"
            );
        }
        let after = table_of(&s, p1);
        // keys kept, the file's rows appended
        let mut want_rows = before.live_rows();
        if append {
            assert_eq!(after.live_rows().len(), want_rows.len() + 1, "{why}");
            want_rows.push(after.live_rows().last().unwrap().clone());
        }
        assert_eq!(after.live_rows(), want_rows, "{why}: row keys");
        assert_eq!(after.live_cols(), before.live_cols(), "{why}: column keys");
        for r in before.live_rows() {
            for c in before.live_cols() {
                let at = (r.clone(), c.clone());
                // (a held cell is the file's field: an empty field and an absent cell render the
                // same bytes, and which one the row has depends on the replica, not the rule)
                match held.get(&at) {
                    Some(v) => assert_eq!(
                        after.cell(&r, &c).unwrap_or(""),
                        v.as_deref().unwrap_or(""),
                        "{why}: {r}/{c}"
                    ),
                    None if whole_rows.contains(&r) => match file_field(&r, &c) {
                        Some(v) if v.is_empty() => assert_eq!(
                            after.cell(&r, &c).unwrap_or(""),
                            "",
                            "{why}: {r}/{c} (a whole row: the file's empty field)"
                        ),
                        v => assert_eq!(
                            after.cell(&r, &c).map(str::to_owned),
                            v,
                            "{why}: {r}/{c} (a whole row: the file's field)"
                        ),
                    },
                    None => assert_eq!(after.cell(&r, &c), want.cell(&r, &c), "{why}: {r}/{c}"),
                }
                let Some(b) = s.blame_cell(p1, &r, &c).unwrap() else {
                    continue;
                };
                let file_changed = file_set
                    .get(&at)
                    .is_some_and(|v| Some(v.as_str()) != before.cell(&r, &c))
                    || overridden.iter().any(|(x, y, _)| *x == r && *y == c)
                    // a whole row's cell whose stored state the save changed (absent to `""`
                    // counts) is the file's; one left unchanged keeps its blame
                    || (whole_rows.contains(&r) && after.cell(&r, &c) != before.cell(&r, &c));
                let want_author = if file_changed {
                    k2_mpc_base::session::EXTERNAL_PID.to_owned()
                } else {
                    blame_before
                        .get(&(r.clone(), c.clone()))
                        .cloned()
                        .unwrap_or_default()
                };
                assert_eq!(b.author, want_author, "{why}: blame of {r}/{c}");
            }
        }
        // one note and one event per overridden live value; the version holds them
        // (one note per row, listing its cells)
        let notes = overwritten(&s.take_notes(p1).unwrap());
        let ow = cells_of(&notes);
        assert_eq!(ow.len(), overridden.len(), "{why}: notes {notes:?}");
        assert_eq!(f.overrides().len(), overridden.len(), "{why}: audit events");
        for (r, c, lv) in &overridden {
            let o = ow
                .iter()
                .find(|o| o.0 == format!("{r}/{c}"))
                .unwrap_or_else(|| panic!("{why}: no note for {r}/{c}"));
            assert_eq!(o.1, "cell");
            assert_eq!(o.2, format!("{lv:?}"), "{why}");
            let vid = o.4.expect("a version");
            assert_eq!(
                version_holds(&s, p1, vid, r.as_str(), c.as_str()).as_deref(),
                Some(lv.as_str()),
                "{why}: the live value is in history"
            );
        }
        // the bytes after the write-back
        f.advance(2_000);
        s.flush(p1).unwrap();
        let mut want_t = want.clone();
        if append {
            let nk = after.live_rows().last().unwrap().clone();
            let cols = want_t.live_cols();
            for c in &cols {
                let _ = want_t.set_cell(&nk, c, "new");
            }
        }
        let _ = want_t;
        assert_eq!(f.bytes(), render(&after), "{why}: the file is the merge");
        assert_eq!(s.audit_disk_sync().unwrap(), vec![], "{why}: K-AUD-1 after");
        assert!(s.audit().unwrap().is_clean(), "{why}: {:?}", s.audit());
    }

    #[test]
    fn grid_an_outside_save_amid_any_mix_of_changes_is_merged() {
        let n = cases();
        let mut runner = proptest::test_runner::TestRunner::new(seeded(
            n,
            "grid_an_outside_save_amid_any_mix_of_changes_is_merged",
        ));
        let grid = (2usize..6, 3usize..5);
        let neutral = prop::sample::select(vec![
            Neutral::None,
            Neutral::HeaderOnOff,
            Neutral::UnsetEmpty,
            Neutral::DropDeleted,
            Neutral::CellSetBack,
            Neutral::CellSetUndo,
            Neutral::RowDeleteUndo,
            Neutral::RowMoveUndo,
            Neutral::Revert,
            Neutral::Restore,
        ]);
        let between =
            prop::sample::select(vec![Between::Nothing, Between::Flush, Between::Restart]);
        let edits = prop::collection::vec((0usize..6, 0usize..5, 0usize..3), 0..3);
        let strategy = (
            grid,
            neutral,
            between,
            edits.clone(),
            prop::collection::vec((0usize..6, 0usize..5, 0usize..3), 1..3),
            any::<bool>(),
        );
        // shrunk failing inputs, replayed first (proptest's regressions file keeps a failing
        // case's RNG seed, and these predate the printed seed): base-a2 54b45362 at f5931e3
        case(
            (5, 3),
            Neutral::UnsetEmpty,
            Between::Nothing,
            &[(5, 1, 0)],
            &[(5, 0, 0)],
            false,
        );
        // the draft's exact compare of an empty field after a write-back (job bf6f7c37): the
        // file's parse keeps the base's absent cell
        case(
            (2, 4),
            Neutral::UnsetEmpty,
            Between::Flush,
            &[(0, 1, 0)],
            &[(2, 1, 0)],
            false,
        );
        // and the (5,4) one behind the model's whole-row flip (job 5eb39bf1's input)
        case(
            (5, 4),
            Neutral::UnsetEmpty,
            Between::Nothing,
            &[(3, 0, 0), (5, 1, 0)],
            &[(5, 3, 0)],
            true,
        );
        if let Err(e) = runner.run(&strategy, |(seed, ne, be, live, out, app)| {
            case(seed, ne, be, &live, &out, app);
            Ok(())
        }) {
            panic!("{e}");
        }
        println!(
            "RUNREC {{\"job\":null,\"run\":\"grid_outside_merge\",\"tool\":\"proptest\",\"status\":\"PASSED\",\"cases\":{n}}}"
        );
    }

    /// The overlap cases always: the same cell after each byte-neutral change.
    #[test]
    fn the_overlap_after_each_neutral_change() {
        for ne in [
            Neutral::None,
            Neutral::HeaderOnOff,
            Neutral::UnsetEmpty,
            Neutral::DropDeleted,
            Neutral::CellSetBack,
        ] {
            for be in [Between::Nothing, Between::Flush, Between::Restart] {
                case(
                    (3, 3),
                    ne,
                    be,
                    &[(1, 1, 0), (2, 0, 0)],
                    &[(1, 1, 1), (0, 1, 2)],
                    true,
                );
            }
        }
    }

    // ---------- re-identification with edits pending (C1 must-have: the identity gap) ----------

    /// A sequence that changes no byte but may give elements new keys.
    #[derive(Debug, Clone, Copy)]
    pub(super) enum Ident {
        /// header on, then off: the first row becomes names, then a new row
        HeaderOnOff,
        /// a row deleted and inserted again with its values, in its place
        RowReinsert,
        /// a column deleted and inserted again with its values, in its place
        ColReinsert,
        /// a row moved to the end and back
        RowMoveBack,
        /// a column moved to the end and back
        ColMoveBack,
        /// a row deleted, then the delete undone
        RowDeleteUndo,
        /// a row deleted, `drop-deleted`, then inserted again with its values
        DropReinsert,
        /// a version named, a row deleted and dropped, then the version restored (the row
        /// comes back under a fresh key)
        RestoreAfterDrop,
        /// a column deleted, `drop-deleted`, then inserted again with its values
        DropColReinsert,
    }

    #[derive(Debug, Clone, Copy)]
    pub(super) enum Step {
        /// a pending cell edit at (line, field) by p1 (`false`) or p2 (`true`)
        Edit(usize, usize, bool),
        Ident(Ident, usize),
    }

    /// The live element at a rendered line and field (the layout never changes here).
    fn at_line(s: &Session, c: ConnId, line: usize, field: usize) -> (Option<Key>, Key) {
        let t = table_of(s, c);
        let cols = t.live_cols();
        let col = cols[field % cols.len()].clone();
        let lm = t.line_map();
        let row = lm.row_at(line).cloned();
        (row, col)
    }

    /// Run one identity sequence at line `i` (field `i` for columns).
    fn ident(s: &Session, p: ConnId, id: Ident, i: usize) -> Result<(), String> {
        let o = EditOptions::new();
        let t = table_of(s, p);
        let rows = t.live_rows();
        let cols = t.live_cols();
        // data rows only: the header line has no row
        let row_i = |i: usize| rows[i % rows.len()].clone();
        let col_j = |j: usize| cols[j % cols.len()].clone();
        let e = |r: Result<u64, SessionError>| r.map(|_| ()).map_err(|e| e.to_string());
        match id {
            Ident::HeaderOnOff => {
                if t.header {
                    return Ok(());
                }
                e(op(s, p, &o, |t| t.set_header(true)))?;
                e(op(s, p, &o, |t| t.set_header(false)))?;
            }
            Ident::RowReinsert | Ident::DropReinsert => {
                if rows.len() < 2 {
                    return Ok(());
                }
                let r = row_i(i);
                let at = match rows.iter().position(|x| *x == r).unwrap() {
                    0 => Anchor::Start,
                    n => Anchor::After(rows[n - 1].clone()),
                };
                let vals: Vec<String> = cols
                    .iter()
                    .map(|c| t.cell(&r, c).unwrap_or("").to_owned())
                    .collect();
                e(op(s, p, &o, |t| t.delete_row(&r)))?;
                if matches!(id, Ident::DropReinsert) {
                    s.drop_deleted(p).map_err(|e| e.to_string())?;
                }
                e(op(s, p, &o, |t| t.insert_row(at, &vals).map(|x| x.1)))?;
            }
            Ident::ColReinsert | Ident::DropColReinsert => {
                if cols.len() < 2 {
                    return Ok(());
                }
                let c = col_j(i);
                let at = match cols.iter().position(|x| *x == c).unwrap() {
                    0 => Anchor::Start,
                    n => Anchor::After(cols[n - 1].clone()),
                };
                let vals: Vec<(Key, String)> = rows
                    .iter()
                    .filter_map(|r| Some((r.clone(), t.cell(r, &c)?.to_owned())))
                    .collect();
                let name = t.name(&c).map(str::to_owned);
                e(op(s, p, &o, |t| t.delete_col(&c)))?;
                if matches!(id, Ident::DropColReinsert) {
                    s.drop_deleted(p).map_err(|e| e.to_string())?;
                }
                e(op(s, p, &o, move |t| {
                    let (k, mut out) = t.insert_col(at, name.as_deref())?;
                    for (r, v) in &vals {
                        out.extend(t.set_cell(r, &k, v)?);
                    }
                    Ok(out)
                }))?;
            }
            Ident::RowMoveBack => {
                if rows.len() < 2 {
                    return Ok(());
                }
                let r = row_i(i);
                let back = match rows.iter().position(|x| *x == r).unwrap() {
                    0 => Anchor::Start,
                    n => Anchor::After(rows[n - 1].clone()),
                };
                e(op(s, p, &o, |t| t.move_row(&r, Anchor::End)))?;
                e(op(s, p, &o, |t| t.move_row(&r, back)))?;
            }
            Ident::ColMoveBack => {
                if cols.len() < 2 {
                    return Ok(());
                }
                let c = col_j(i);
                let back = match cols.iter().position(|x| *x == c).unwrap() {
                    0 => Anchor::Start,
                    n => Anchor::After(cols[n - 1].clone()),
                };
                e(op(s, p, &o, |t| t.move_col(&c, Anchor::End)))?;
                e(op(s, p, &o, |t| t.move_col(&c, back)))?;
            }
            Ident::RowDeleteUndo => {
                if rows.len() < 2 {
                    return Ok(());
                }
                let r = row_i(i);
                e(op(s, p, &o, |t| t.delete_row(&r)))?;
                let u = s
                    .undo_with(p, &UndoRequest::steps(1).mode(UndoMode::All))
                    .map_err(|e| e.to_string())?;
                if !matches!(u, UndoOutcome::Applied { .. }) {
                    return Err(format!("the undo of the delete: {u:?}"));
                }
            }
            Ident::RestoreAfterDrop => {
                if rows.len() < 2 {
                    return Ok(());
                }
                let v = s
                    .name_version(p, "before", None)
                    .map_err(|e| e.to_string())?;
                let r = row_i(i);
                e(op(s, p, &o, |t| t.delete_row(&r)))?;
                s.drop_deleted(p).map_err(|e| e.to_string())?;
                s.restore(p, v).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// One case: a grid; a mix of pending cell edits (two people) and identity sequences;
    /// then an outside save (cells rewritten, maybe a line appended). The §2.9.1 oracle works
    /// on lines and fields, not keys (every sequence keeps the layout): the live file is the
    /// synced file with the pending values, then the file's values over them; every field the
    /// file changed blames to `kit:external`, every pending-only field to its writer; one
    /// note (to that writer) and one audit event per field both changed; every row once (no
    /// duplicate); after the write-back the file is exactly that, and the audits are clean.
    pub(super) fn ident_case(
        (nr, nc): (usize, usize),
        steps: &[Step],
        outside: &[(usize, usize)],
        append: bool,
    ) {
        let why = format!("{nr}x{nc} {steps:?} outside {outside:?} append {append}");
        let lines0: Vec<Vec<String>> = (0..nr)
            .map(|i| (0..nc).map(|j| format!("v{i}x{j}")).collect())
            .collect();
        let text = |l: &[Vec<String>]| -> String {
            l.iter().map(|r| format!("{}\n", r.join(","))).collect()
        };
        let synced = text(&lines0);
        let f = Fix::new();
        std::fs::write(f.file(), &synced).unwrap();
        let s = f.open();
        let p1 = f.join(&s, "p1");
        let p2 = f.join(&s, "p2");
        let mut pending: HashMap<(usize, usize), String> = HashMap::new();
        // who wrote each field last: a pending edit's writer, or p1 when a sequence wrote the
        // field again (a row or column inserted again, a header made a row, a restore)
        let mut author: HashMap<(usize, usize), &str> = HashMap::new();
        // the dropped rows holding a value someone else wrote: `(who, value)`, from the case's
        // own record of writers
        // `(who, value, rule, line)`: a cell a delete hid, and the note rule a drop gives it (a
        // dropped row's cells: `dropped_row`; a dropped column's in a row that stays:
        // `dropped_col`; a cell in both a dropped row and a dropped column: `dropped_row`)
        let mut expect_drops: Vec<(String, String, &str, usize)> = Vec::new();
        let mut hidden: Vec<(String, String, &str, usize)> = Vec::new();
        for (n, st) in steps.iter().enumerate() {
            match *st {
                Step::Edit(i, j, second) => {
                    let (line, field) = (i % nr + 1, j % nc);
                    let (who, c) = if second { ("p2", p2) } else { ("p1", p1) };
                    let v = format!("P{n}");
                    let (row, col) = at_line(&s, c, line, field);
                    let r = match row {
                        Some(r) => op(&s, c, &EditOptions::new(), |t| t.set_cell(&r, &col, &v)),
                        // the header line: a column's name
                        None => op(&s, c, &EditOptions::new(), |t| t.rename_col(&col, &v)),
                    };
                    r.unwrap_or_else(|e| panic!("{why}: step {n}: {e}"));
                    pending.insert((line, field), v);
                    author.insert((line, field), who);
                }
                Step::Ident(id, i) => {
                    // a sequence that leaves a row deleted leaves its cells hidden there; one
                    // that drops takes every such row
                    // (a row's fields, or a column's: the cells a delete hides)
                    let (at, rule): (Vec<(usize, usize)>, &str) = match id {
                        Ident::HeaderOnOff => ((0..nc).map(|f| (1, f)).collect(), "dropped_row"),
                        Ident::RowReinsert | Ident::DropReinsert | Ident::RestoreAfterDrop
                            if nr >= 2 =>
                        {
                            ((0..nc).map(|f| (i % nr + 1, f)).collect(), "dropped_row")
                        }
                        Ident::ColReinsert | Ident::DropColReinsert if nc >= 2 => {
                            ((1..=nr).map(|l| (l, i % nc)).collect(), "dropped_col")
                        }
                        _ => (Vec::new(), ""),
                    };
                    if rule == "dropped_row" {
                        // a hidden column cell whose row is now deleted too: in a dropped row
                        let line = at.first().map(|a| a.0);
                        for h in hidden.iter_mut().filter(|h| Some(h.3) == line) {
                            h.2 = "dropped_row";
                        }
                    }
                    for at in at {
                        if let (Some(who), Some(v)) = (author.get(&at), pending.get(&at)) {
                            if *who != "p1" {
                                hidden.push(((*who).to_owned(), v.clone(), rule, at.0));
                            }
                        }
                    }
                    if matches!(
                        id,
                        Ident::DropReinsert | Ident::RestoreAfterDrop | Ident::DropColReinsert
                    ) {
                        expect_drops.append(&mut hidden);
                    }
                    ident(&s, p1, id, i).unwrap_or_else(|e| panic!("{why}: step {n} {id:?}: {e}"));
                    let fields: Vec<(usize, usize)> = match id {
                        Ident::HeaderOnOff => (0..nc).map(|f| (1, f)).collect(),
                        Ident::RowReinsert | Ident::DropReinsert | Ident::RestoreAfterDrop => {
                            (0..nc).map(|f| (i % nr + 1, f)).collect()
                        }
                        Ident::ColReinsert | Ident::DropColReinsert => {
                            (1..=nr).map(|l| (l, i % nc)).collect()
                        }
                        _ => Vec::new(),
                    };
                    for k in fields {
                        author.insert(k, "p1");
                    }
                }
            }
        }
        // the layout is the synced one, with the pending values
        let mut want: Vec<Vec<String>> = lines0.clone();
        for ((line, field), v) in &pending {
            want[line - 1][*field] = v.clone();
        }
        assert_eq!(
            String::from_utf8(s.read(p1).unwrap().bytes).unwrap(),
            text(&want),
            "{why}: the sequences moved no byte"
        );
        assert_eq!(f.bytes(), synced, "{why}: nothing written yet");
        // the outside save
        let mut saved = lines0.clone();
        let mut file_set: HashMap<(usize, usize), String> = HashMap::new();
        for (n, (i, j)) in outside.iter().enumerate() {
            let (line, field) = (i % nr + 1, j % nc);
            // one field per line: the aligner keeps every line on its row (§2.9 step 3)
            if file_set.keys().any(|(l, _)| *l == line) {
                continue;
            }
            let v = format!("F{n}");
            saved[line - 1][field] = v.clone();
            file_set.insert((line, field), v);
        }
        if append {
            saved.push(vec!["new".to_owned(); nc]);
        }
        let check = f.save(&s, &text(&saved));
        assert!(
            matches!(check, DiskCheck::Ingested { .. }),
            "{why}: {check:?}"
        );
        // the oracle
        // a line the file changed is the file's whole (contract §2.9.1): every pending field on
        // it that differs from the file's is overridden; one equal to the file's overwrites
        // nothing, so it has no note. Other lines keep their pending fields
        let mut expect_notes: Vec<(String, String)> = Vec::new();
        let mut overridden_fields: Vec<(usize, usize)> = Vec::new();
        for line in 1..=nr {
            if saved[line - 1] == lines0[line - 1] {
                continue;
            }
            for field in 0..nc {
                let fv = &saved[line - 1][field];
                if let Some(pv) = pending.get(&(line, field)) {
                    if pv != fv {
                        let who = author[&(line, field)];
                        expect_notes.push((who.to_string(), format!("{pv:?}")));
                        overridden_fields.push((line, field));
                    }
                }
                want[line - 1][field] = fv.clone();
            }
        }
        if append {
            want.push(vec!["new".to_owned(); nc]);
        }
        let live = String::from_utf8(s.read(p1).unwrap().bytes).unwrap();
        assert_eq!(live, text(&want), "{why}: the merge");
        let t = table_of(&s, p1);
        assert_eq!(
            t.live_rows().len(),
            want.len() - usize::from(t.header),
            "{why}: rows once"
        );
        // blame by line and field
        let lm = t.line_map();
        let live_cols = t.live_cols();
        for line in 1..=nr {
            let Some(r) = lm.row_at(line) else { continue };
            for (field, c) in live_cols.iter().enumerate() {
                let Some(b) = s.blame_cell(p1, r, c).unwrap() else {
                    continue;
                };
                let want_author = if overridden_fields.contains(&(line, field))
                    || file_set
                        .get(&(line, field))
                        .is_some_and(|v| *v != lines0[line - 1][field])
                {
                    k2_mpc_base::session::EXTERNAL_PID.to_owned()
                } else if let Some(who) = author.get(&(line, field)) {
                    (*who).to_owned()
                } else {
                    continue; // the import
                };
                assert_eq!(
                    b.author, want_author,
                    "{why}: blame of L{line} field {field}"
                );
            }
        }
        // notes and events
        let mut got: Vec<(String, String)> = Vec::new();
        for (who, c) in [("p1", p1), ("p2", p2)] {
            // the save's notes are checked as ever; a drop's note, by the case's own record
            let (drops, saves): (Vec<Overwrite>, Vec<Overwrite>) =
                overwritten(&s.take_notes(c).unwrap())
                    .into_iter()
                    .partition(|o| o.source == "rows drop-deleted");
            for o in cells_of(&saves) {
                assert_eq!(o.1, "cell", "{why}: {o:?}");
                assert!(o.4.is_some(), "{why}: {o:?}");
                got.push((who.to_owned(), o.2.clone()));
            }
            let want_drops: Vec<(&String, &str)> = expect_drops
                .iter()
                .filter(|(w, ..)| w == who)
                .map(|(_, v, r, _)| (v, *r))
                .collect();
            for (v, rule) in &want_drops {
                assert!(
                    drops
                        .iter()
                        .any(|o| o.rule == *rule && o.yours.contains(v.as_str())),
                    "{why}: no {rule} note holds {v} for {who}: {drops:?}"
                );
            }
            assert_eq!(
                drops.is_empty(),
                want_drops.is_empty(),
                "{why}: drop notes for {who}: {drops:?}, want {want_drops:?}"
            );
        }
        got.sort();
        expect_notes.sort();
        assert_eq!(got, expect_notes, "{why}: the notes");
        assert_eq!(
            f.overrides().len(),
            expect_notes.len(),
            "{why}: audit events {:?}",
            f.overrides()
        );
        // the write-back, the audits
        f.advance(2_000);
        s.flush(p1).unwrap();
        assert_eq!(f.bytes(), text(&want), "{why}: the file");
        assert_eq!(s.audit_disk_sync().unwrap(), vec![], "{why}: K-AUD-1");
        assert!(s.audit().unwrap().is_clean(), "{why}: {:?}", s.audit());
    }

    fn ident_cases() -> u32 {
        std::env::var("GRID_IDENT_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(128)
    }

    /// The identity gap closed: byte-neutral sequences that re-key rows or columns, with
    /// edits pending before, between and after them, then an outside save, against the
    /// §2.9.1 oracle.
    #[test]
    fn grid_outside_save_after_reidentification_with_edits_pending() {
        let n = ident_cases();
        let mut runner = proptest::test_runner::TestRunner::new(seeded(
            n,
            "grid_outside_save_after_reidentification_with_edits_pending",
        ));
        let ident = prop::sample::select(vec![
            Ident::HeaderOnOff,
            Ident::RowReinsert,
            Ident::ColReinsert,
            Ident::RowMoveBack,
            Ident::ColMoveBack,
            Ident::RowDeleteUndo,
            Ident::DropReinsert,
            Ident::RestoreAfterDrop,
            Ident::DropColReinsert,
        ]);
        let step = prop_oneof![
            (0usize..6, 0usize..4, any::<bool>()).prop_map(|(i, j, b)| Step::Edit(i, j, b)),
            (ident, 0usize..6).prop_map(|(id, i)| Step::Ident(id, i)),
        ];
        let strategy = (
            (2usize..6, 2usize..5),
            prop::collection::vec(step, 1..7),
            prop::collection::vec((0usize..6, 0usize..4), 1..4),
            any::<bool>(),
        );
        if let Err(e) = runner.run(&strategy, |(shape, steps, out, app)| {
            ident_case(shape, &steps, &out, app);
            Ok(())
        }) {
            panic!("{e}");
        }
        println!(
            "RUNREC {{\"job\":null,\"run\":\"grid_outside_reidentified\",\"tool\":\"proptest\",\"status\":\"PASSED\",\"cases\":{n}}}"
        );
    }

    /// The hand mutant's minimal case (columns not re-identified: the save duplicated a
    /// column), kept as a regression.
    #[test]
    fn a_column_dropped_and_inserted_again_then_a_save() {
        ident_case(
            (2, 2),
            &[
                Step::Edit(0, 0, false),
                Step::Ident(Ident::DropColReinsert, 0),
            ],
            &[(0, 0)],
            false,
        );
    }

    /// Every sequence once, with an edit pending on each side of it and a save over the same
    /// row and others.
    #[test]
    fn each_reidentification_then_a_save() {
        for id in [
            Ident::HeaderOnOff,
            Ident::RowReinsert,
            Ident::ColReinsert,
            Ident::RowMoveBack,
            Ident::ColMoveBack,
            Ident::RowDeleteUndo,
            Ident::DropReinsert,
            Ident::RestoreAfterDrop,
            Ident::DropColReinsert,
        ] {
            for at in 0..3 {
                ident_case(
                    (3, 3),
                    &[
                        Step::Edit(2, 2, false),
                        Step::Ident(id, at),
                        Step::Edit(at, 1, true),
                    ],
                    &[(at, 0), (at + 1, 1), (2, 2)],
                    true,
                );
            }
        }
    }

    /// A text against concurrent cell edits (v0.10.0, contract §3.4, §2.9.1): `write --base` or
    /// an outside save turns row A into A', often like another row B, while a person edits
    /// cells live. Pairing is a guess about identity, so safety doesn't rest on it. The oracle
    /// doesn't use the reconcile: every row of the result is a whole line of the text, or its
    /// own key's live row where the text left the row exact (exact twins can't be told apart);
    /// a live value that didn't survive has a note whose version holds it; a row only one
    /// side changed never conflicts.
    mod row_pairing {
        use super::*;
        use proptest::strategy::ValueTree;

        #[derive(Debug, Clone, Copy, PartialEq)]
        enum Pick {
            /// A's own value.
            Keep,
            /// B's value in the column.
            Borrow,
            /// A value no row has.
            Fresh,
            Blank,
        }

        #[derive(Debug, Clone, Copy, PartialEq)]
        enum OursB {
            Keep,
            Delete,
            /// B's line becomes values no row has.
            Rewrite,
        }

        #[derive(Debug, Clone, Copy, PartialEq)]
        enum Form {
            WriteBase,
            WriteBasePartial,
            Outside,
        }

        #[derive(Debug, Clone)]
        struct Spec {
            width: usize,
            /// base rows (B's twin, when there is one, is appended)
            rows: usize,
            /// per base cell (row-major): blank
            blank: Vec<bool>,
            /// per column: one value every base row shares (ties)
            shared: Vec<bool>,
            a: usize,
            b: usize,
            picks: Vec<Pick>,
            ours_b: OursB,
            /// a row equal to B but in its last column (a tie for A')
            twin: bool,
            /// the twin's line in the text: kept, or deleted
            twin_deleted: bool,
            /// B's columns the person sets live
            live_b: Vec<usize>,
            /// one of A's columns the person sets live too
            live_a: Option<usize>,
            /// a new line right before A', like A but in the columns of this mask (a row
            /// inserted where A was: a pairing by place could give it A's key)
            insert: Option<u8>,
            /// two lines of the text change places (by index, modulo the lines), changed or not
            swap: Option<(u8, u8)>,
            /// a row other than A and B the person edits live, one column
            live_other: Option<(u8, u8)>,
            /// the text deletes the last column (every row changed, K2 Cores' item 3 ruling)
            del_col: bool,
            /// with a column delete, a live value in that column, on this row (mod the rows)
            live_del: Option<u8>,
            /// a live column change after the live cell edits: (a delete, else a move to the
            /// start; which column, mod the width)
            live_col: Option<(bool, u8)>,
            form: Form,
        }

        fn cases() -> u32 {
            std::env::var("ROW_PAIRING_CASES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(256)
        }

        fn spec() -> impl Strategy<Value = Spec> {
            (2usize..6, 2usize..6).prop_flat_map(|(width, rows)| {
                let pick = prop_oneof![
                    5 => Just(Pick::Borrow),
                    2 => Just(Pick::Keep),
                    2 => Just(Pick::Fresh),
                    2 => Just(Pick::Blank),
                ];
                (
                    prop::collection::vec(prop::bool::weighted(0.15), rows * width),
                    prop::collection::vec(prop::bool::weighted(0.15), width),
                    (0..rows, 1..rows),
                    prop::collection::vec(pick, width),
                    prop::sample::select(vec![OursB::Keep, OursB::Delete, OursB::Rewrite]),
                    (any::<bool>(), any::<bool>()),
                    // often no live edit on B or A: the clean merges (over-refusal is a failure
                    // too, and needs rows only one side changed)
                    prop::collection::vec(0..width, 0..3),
                    (
                        prop::bool::weighted(0.15),
                        prop::option::weighted(0.4, any::<u8>()),
                        prop::option::weighted(0.2, any::<(bool, u8)>()),
                    ),
                    (
                        prop::option::weighted(0.25, 0..width),
                        prop::option::weighted(0.3, any::<u8>()),
                        prop::option::weighted(0.4, any::<(u8, u8)>()),
                        prop::option::weighted(0.7, any::<(u8, u8)>()),
                    ),
                )
                    .prop_map(
                        move |(
                            blank,
                            shared,
                            (a, db),
                            picks,
                            ours_b,
                            (twin, twin_deleted),
                            live_b,
                            (del_col, live_del, live_col),
                            (live_a, insert, swap, live_other),
                        )| Spec {
                            width,
                            rows,
                            blank,
                            shared,
                            a,
                            b: (a + db) % rows,
                            picks,
                            ours_b,
                            twin,
                            twin_deleted,
                            live_b,
                            live_a,
                            insert,
                            swap,
                            live_other,
                            del_col: del_col && width > 2,
                            live_del,
                            live_col,
                            // (both set per cell of the survey)
                            form: Form::WriteBase,
                        },
                    )
            })
        }

        type Row = Vec<String>;

        fn line(r: &Row) -> String {
            format!("{}\n", r.join(","))
        }

        type Built = (
            Vec<Row>,
            Vec<(Option<usize>, Row)>,
            Vec<(usize, usize, String)>,
        );

        /// The base rows, the text's lines with the base row each came from (`None`: a line
        /// no base row made), and the live cell edits as (base row, column, value).
        fn build(sp: &Spec) -> Built {
            let w = sp.width;
            let mut base: Vec<Row> = (0..sp.rows)
                .map(|i| {
                    (0..w)
                        .map(|c| {
                            if sp.shared[c] {
                                format!("s{c}")
                            } else if sp.blank[i * w + c] {
                                String::new()
                            } else {
                                format!("v{i}x{c}")
                            }
                        })
                        .collect()
                })
                .collect();
            let (a, b) = (sp.a, sp.b);
            if sp.twin {
                let mut t = base[b].clone();
                t[w - 1] = format!("t{}", w - 1);
                base.push(t);
            }
            let a2: Row = (0..w)
                .map(|c| match sp.picks[c] {
                    Pick::Keep => base[a][c].clone(),
                    Pick::Borrow => base[b][c].clone(),
                    Pick::Fresh => format!("n{c}"),
                    Pick::Blank => String::new(),
                })
                .collect();
            let mut ours = Vec::new();
            for (i, r) in base.iter().enumerate() {
                if i == a {
                    if let Some(mask) = sp.insert {
                        let n: Row = (0..w)
                            .map(|c| {
                                if mask & (1 << c) != 0 {
                                    format!("i{c}")
                                } else {
                                    base[a][c].clone()
                                }
                            })
                            .collect();
                        ours.push((None, n));
                    }
                    ours.push((Some(i), a2.clone()));
                } else if i == b {
                    match sp.ours_b {
                        OursB::Keep => ours.push((Some(i), r.clone())),
                        OursB::Delete => {}
                        OursB::Rewrite => {
                            ours.push((Some(i), (0..w).map(|c| format!("o{c}")).collect()))
                        }
                    }
                } else if sp.twin && i == base.len() - 1 && sp.twin_deleted {
                } else {
                    ours.push((Some(i), r.clone()));
                }
            }
            // the text deletes the LAST column: every field index below it stays the base's, which
            // the risky-shape count relies on (see `case`)
            if sp.del_col {
                for (_, l) in ours.iter_mut() {
                    l.pop();
                }
            }
            if let Some((x, y)) = sp.swap {
                let (x, y) = (x as usize % ours.len(), y as usize % ours.len());
                ours.swap(x, y);
            }
            let mut live: Vec<(usize, usize, String)> = Vec::new();
            for &c in &sp.live_b {
                if !live.iter().any(|(_, x, _)| *x == c) {
                    live.push((b, c, format!("T{c}")));
                }
            }
            if let Some(c) = sp.live_a {
                live.push((a, c, format!("U{c}")));
            }
            if let (true, Some(r)) = (sp.del_col, sp.live_del) {
                let r = r as usize % base.len();
                if !live.iter().any(|(i, x, _)| *i == r && *x == w - 1) {
                    live.push((r, w - 1, format!("D{}", w - 1)));
                }
            }
            if let Some((r, c)) = sp.live_other {
                let (r, c) = (r as usize % base.len(), c as usize % w);
                if r != a && r != b && !live.iter().any(|(i, x, _)| *i == r && *x == c) {
                    live.push((r, c, format!("W{c}")));
                }
            }
            (base, ours, live)
        }

        fn rows_by_key(t: &Table) -> Vec<(Key, Row)> {
            let cols = t.live_cols();
            t.live_rows()
                .into_iter()
                .map(|r| {
                    let cells = cols
                        .iter()
                        .map(|c| t.cell(&r, c).unwrap_or("").to_owned())
                        .collect();
                    (r, cells)
                })
                .collect()
        }

        /// What the case did: whether the rewrite applied (anything), and whether A' shares
        /// at least half its non-empty fields with B (the risky shape).
        struct Ran {
            applied: bool,
            risky: bool,
            /// Byte-identical rows, in the base or among the text's lines (checked from the case's
            /// own rows, not the kit's pairing): out of the over-refusal check, a stated limit.
            twins: bool,
            /// the text deleted a column and the write conflicted (item 3's cost)
            col_del_refused: bool,
            /// a live value in the column the text deleted
            live_in_deleted_col: bool,
            /// a live column change, and the write conflicted
            live_col_refused: bool,
        }

        fn case(sp: &Spec) -> Result<Ran, String> {
            let (base, ours, live) = build(sp);
            let f = Fix::new();
            let base_text: String = base.iter().map(line).collect();
            std::fs::write(f.file(), &base_text).unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            let bot = s
                .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                .unwrap()
                .conn;
            let keys = table_of(&s, p).live_rows();
            if keys.len() != base.len() {
                return Err(format!(
                    "import keyed {} rows of {}",
                    keys.len(),
                    base.len()
                ));
            }
            let read = s.read(bot).unwrap().version;
            for (i, c, v) in &live {
                set(&s, p, keys[*i].as_str(), &format!("c{}", c + 1), v);
            }
            // the live column change (a delete leaves at least two columns)
            let live_col: Option<(bool, Key)> = sp.live_col.and_then(|(del, c)| {
                let ck = k(&format!("c{}", c as usize % sp.width + 1));
                if del && sp.width < 3 {
                    return None;
                }
                let r = if del {
                    op(&s, p, &EditOptions::new(), |t| t.delete_col(&ck))
                } else {
                    op(&s, p, &EditOptions::new(), |t| {
                        t.move_col(&ck, Anchor::Start)
                    })
                };
                r.ok().map(|_| (del, ck))
            });
            let theirs_t = table_of(&s, p);
            let theirs = rows_by_key(&theirs_t);
            let text: String = ours.iter().map(|(_, r)| line(r)).collect();
            // `clean`: everything applied, nothing refused or left out
            let (clean, conflicted, said) = match sp.form {
                Form::WriteBase | Form::WriteBasePartial => {
                    let opts = if sp.form == Form::WriteBasePartial {
                        GridWriteOptions::default().partial()
                    } else {
                        GridWriteOptions::default()
                    };
                    let r = s
                        .write_base_grid(bot, read, text.as_bytes(), &opts)
                        .unwrap();
                    let list = |c: &[k2_mpc_base::session::GridConflict]| {
                        c.iter().map(|x| x.to_string()).collect::<Vec<_>>()
                    };
                    match &r.outcome {
                        GridWrite::Applied(_) if r.skipped.is_empty() => {
                            (true, false, String::new())
                        }
                        GridWrite::Applied(_) => (false, true, format!("{:?}", list(&r.skipped))),
                        GridWrite::Conflicts(c) => (false, true, format!("{:?}", list(c))),
                        o => return Err(format!("unexpected outcome {o:?}")),
                    }
                }
                Form::Outside => {
                    f.save(&s, &text);
                    (true, false, String::new())
                }
            };
            let result_t = table_of(&s, p);
            let result = rows_by_key(&result_t);
            // the author's own queue (every live value here is p's), read once
            let notes = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            // a row the text's column layout cut: its own live row on the result's columns, and
            // every cell of a column that went is empty, or noted to its author with exactly that
            // value and still in the hidden column (K2 Cores' form)
            // a base cell's value at V, by row and column key (`None`: no base row)
            let base_at = |r: &Key, c: &Key| -> Option<String> {
                let i = keys.iter().position(|x| x == r)?;
                let j = c.as_str()[1..].parse::<usize>().ok()? - 1;
                Some(base[i].get(j).cloned().unwrap_or_default())
            };
            let live_wrote = |r: &Key, c: &Key| {
                live.iter()
                    .any(|(i, j, _)| keys[*i] == *r && format!("c{}", j + 1) == c.as_str())
            };
            // a row the text's column layout cut: its own live row on the result's columns, and
            // each cell of a column that went either unchanged from V (nobody wrote it live) or,
            // in an outside save only, a live write noted to its author with exactly that value;
            // either way still in the hidden column (K2 Cores' corrected form)
            let mut by_unchanged = 0u32;
            let mut by_note = 0u32;
            let mut projected_ok = |r: &Key| {
                let on_cols = result_t.live_cols().iter().all(|c| {
                    result_t.cell(r, c).unwrap_or("") == theirs_t.cell(r, c).unwrap_or("")
                });
                if !on_cols {
                    return false;
                }
                for c in theirs_t
                    .live_cols()
                    .into_iter()
                    .filter(|c| !result_t.cols.is_live(c))
                {
                    let lv = theirs_t.cell(r, &c).unwrap_or("").to_owned();
                    if result_t.cell(r, &c).unwrap_or("") != lv {
                        return false;
                    }
                    let unchanged = !live_wrote(r, &c) && base_at(r, &c).unwrap_or_default() == lv;
                    if unchanged {
                        by_unchanged += 1;
                        continue;
                    }
                    // `write --base` and `--partial` never drop a live write, note or not (option
                    // 2 and ruling (a): a column change that meets a live write is held); the
                    // noted branch below is an outside save's only
                    if sp.form != Form::Outside {
                        println!(
                            "DROPPED-UNDER-WRITE-BASE {:?} row {r} col {c} live {lv:?} base {:?}",
                            sp.form,
                            base_at(r, &c)
                        );
                        return false;
                    }
                    let quoted = format!("{lv:?}");
                    let noted = notes.iter().any(|n| {
                        n.0 == format!("col {c}")
                            && n.1 == "edited_col"
                            && n.2.split(", ").any(|x| x == quoted)
                    });
                    if !noted {
                        println!(
                            "DROPPED-UNNOTED row {r} col {c} live {lv:?} base {:?} live_wrote {}",
                            base_at(r, &c),
                            live_wrote(r, &c)
                        );
                        return false;
                    }
                    by_note += 1;
                }
                true
            };
            // the text's lines by column key (field j is base column c(j+1): the text deletes
            // only the last column, so the keys line up)
            let lines: Vec<&Row> = ours.iter().map(|(_, r)| r).collect();
            let is_text_line = |r: &Key| {
                lines.iter().any(|l| {
                    result_t.live_cols().iter().all(|c| {
                        let j = c.as_str()[1..].parse::<usize>().unwrap() - 1;
                        l.get(j).map(String::as_str).unwrap_or("")
                            == result_t.cell(r, c).unwrap_or("")
                    })
                })
            };
            // the oracle, with no reconcile: each row of the result is a whole line of the text,
            // or its own key's live row, and that only where the text left the row exact (or
            // the text didn't apply to it). Never cells of two sources
            for (r, row) in &result {
                if is_text_line(r) {
                    continue;
                }
                if !projected_ok(r) {
                    return Err(format!(
                        "row {r} {row:?} is no line of the text and not its own live row {:?}",
                        theirs.iter().find(|(x, _)| x == r).map(|(_, v)| v)
                    ));
                }
                if clean {
                    let i = keys.iter().position(|x| x == r);
                    let exact = i.is_some_and(|i| lines.iter().any(|l| **l == base[i]));
                    if !exact {
                        return Err(format!(
                            "row {r} {row:?} kept the live row in a clean result, but the text didn't leave it exact"
                        ));
                    }
                }
            }
            if by_unchanged + by_note > 0 {
                println!("PROJECTED by_unchanged={by_unchanged} by_note={by_note}");
            }
            // no silent loss: a live value that didn't survive has its note, and the version the
            // note names holds it
            for (i, c, v) in &live {
                let k = &keys[*i];
                let ck = Key::new(format!("c{}", c + 1)).unwrap();
                // (a column the live side itself deleted holds its own value, hidden: no loss)
                if live_col.as_ref().is_some_and(|(del, x)| *del && *x == ck) {
                    continue;
                }
                let kept = result_t.rows.is_live(k)
                    && result_t.cols.is_live(&ck)
                    && result_t.cell(k, &ck) == Some(v.as_str());
                if kept {
                    continue;
                }
                if sp.form != Form::Outside {
                    return Err(format!("write --base lost live {v} at {k}/c{}", c + 1));
                }
                let cell = format!("{k}/c{}", c + 1);
                let row = format!("row {k}");
                // a column's note counts only as that column's delete (`edited_col`), holding this
                // very value as one of its decoded fields; `notes` are its author's own (every
                // live value here is p's, and `take_notes(p)` is p's queue)
                let col = format!("col c{}", c + 1);
                let quoted = format!("{v:?}");
                let col_note = |n: &(String, &'static str, String, String, Option<u64>)| {
                    n.0 == col && n.1 == "edited_col" && n.2.split(", ").any(|x| x == quoted)
                };
                // a cell's note holds exactly this value; a row's, as one of its fields
                let cell_note = |n: &(String, &'static str, String, String, Option<u64>)| {
                    n.0 == cell && n.2 == quoted
                };
                let row_note = |n: &(String, &'static str, String, String, Option<u64>)| {
                    n.0 == row && n.2.split(", ").any(|x| x == quoted)
                };
                let Some(n) = notes
                    .iter()
                    .find(|n| cell_note(n) || row_note(n) || col_note(n))
                else {
                    return Err(format!(
                        "the save lost live {v} at {cell}, no note: {notes:?}"
                    ));
                };
                let Some(vid) = n.4 else {
                    return Err(format!("the note for {cell} names no version: {n:?}"));
                };
                let held = version_holds(&s, p, vid, k.as_str(), &format!("c{}", c + 1));
                if held.as_deref() != Some(v.as_str()) {
                    return Err(format!("version {vid} holds {held:?} at {cell}, not {v}"));
                }
            }
            // over-refusal: a row only one side changed never conflicts (a line equal to its
            // base row is one the text left alone). Exact twins, in the base or among the
            // text's lines, can't be told apart (a stated limit), so they stay out of this check
            let distinct = (0..base.len()).all(|i| (0..i).all(|j| base[i] != base[j]))
                && (0..lines.len()).all(|i| (0..i).all(|j| lines[i] != lines[j]));
            let only_one_side = live
                .iter()
                .all(|(i, _, _)| lines.iter().any(|l| **l == base[*i]));
            if distinct && only_one_side && conflicted && live_col.is_none() {
                return Err(format!(
                    "a conflict though no row was changed by both: {said}"
                ));
            }
            let a2 = &ours.iter().find(|(o, _)| *o == Some(sp.a)).unwrap().1;
            let nonempty = a2.iter().filter(|v| !v.is_empty()).count();
            // a2 is one field short when the text deleted a column, and index c means the same
            // column in a2 and the base only because that column is the last (`build`). This count
            // only feeds the `risky_applied` statistic: no pass, fail or exclusion depends on it
            assert!(
                a2.len() == sp.width || (sp.del_col && a2.len() + 1 == sp.width),
                "a deleted column must be the last"
            );
            let shared = (0..a2.len())
                .filter(|&c| !a2[c].is_empty() && a2[c] == base[sp.b][c])
                .count();
            Ok(Ran {
                applied: clean,
                risky: shared >= 1 && 2 * shared >= nonempty,
                twins: !distinct,
                col_del_refused: sp.del_col && conflicted,
                live_col_refused: live_col.is_some() && conflicted,
                live_in_deleted_col: sp.del_col && live.iter().any(|(_, c, _)| *c == sp.width - 1),
            })
        }

        /// Every form, the same cases: B kept, deleted or rewritten, by `write --base`,
        /// `write --base --partial` and an outside save. `ROW_PAIRING_CASES` per form, from a
        /// fixed seed; a failure doesn't stop the survey, so the counts are the whole picture.
        #[test]
        fn a_row_rewritten_like_another_never_takes_its_live_cells() {
            under_merge_mutant(|| {
                let n = cases();
                let mut failed = 0u32;
                for ours_b in [OursB::Keep, OursB::Delete, OursB::Rewrite] {
                    for form in [Form::WriteBase, Form::WriteBasePartial, Form::Outside] {
                        let mut runner = proptest::test_runner::TestRunner::deterministic();
                        let (mut applied, mut risky, mut bad, mut twins) = (0u32, 0u32, 0u32, 0u32);
                        let mut col_del_refused = 0u32;
                        let mut live_in_deleted = 0u32;
                        let mut live_col_refused = 0u32;
                        let mut first = None;
                        let mut first_col_del: Option<String> = None;
                        for _ in 0..n {
                            let mut sp = spec()
                                .new_tree(&mut runner)
                                .expect("the strategy has no filter")
                                .current();
                            sp.ours_b = ours_b;
                            sp.form = form;
                            if std::env::var("ROW_PAIRING_TRACE").is_ok() {
                                println!("case {sp:?}");
                            }
                            match case(&sp) {
                                Ok(r) => {
                                    applied += u32::from(r.applied);
                                    risky += u32::from(r.applied && r.risky);
                                    twins += u32::from(r.twins);
                                    col_del_refused += u32::from(r.col_del_refused);
                                    live_in_deleted += u32::from(r.live_in_deleted_col);
                                    live_col_refused += u32::from(r.live_col_refused);
                                }
                                Err(m) => {
                                    bad += 1;
                                    if std::env::var("ROW_PAIRING_ALL_FAILURES").is_ok() {
                                        println!("FAILURE {ours_b:?}/{form:?}: {m}\n{sp:?}");
                                    }
                                    first.get_or_insert(format!("{m}\n{sp:?}"));
                                    if sp.del_col {
                                        first_col_del.get_or_insert(format!("{m}\n{sp:?}"));
                                    }
                                }
                            }
                        }
                        println!(
                        "RUNREC {{\"job\":null,\"run\":\"grid_row_pairing_{ours_b:?}_{form:?}\",\"tool\":\"proptest\",\"status\":\"{}\",\"cases\":{n},\"failures\":{bad},\"applied\":{applied},\"risky_applied\":{risky},\"excluded_twins\":{twins},\"refused_after_a_column_delete\":{col_del_refused},\"live_values_in_deleted_columns\":{live_in_deleted},\"refused_on_a_live_column_change\":{live_col_refused}}}",
                        if bad == 0 { "PASSED" } else { "FAILED" }
                    );
                        if let Some(m) = first {
                            println!("first failure of {ours_b:?}/{form:?}: {m}");
                        }
                        if let Some(m) = first_col_del {
                            println!("first column-delete failure of {ours_b:?}/{form:?}: {m}");
                        }
                        failed += bad;
                    }
                }
                assert_eq!(
                    failed, 0,
                    "cases that spliced rows (the per-form counts above)"
                );
            })
        }

        const FORMS: [Form; 3] = [Form::WriteBase, Form::WriteBasePartial, Form::Outside];

        /// One named shape: `base`, then the live cell sets, then `text` in `form`. The result
        /// by key, and what the write said.
        fn scenario(
            base: &str,
            live: &[(&str, &str, &str)],
            text: &str,
            form: Form,
        ) -> (Vec<(String, String)>, String) {
            let f = Fix::new();
            std::fs::write(f.file(), base).unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            let bot = s
                .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                .unwrap()
                .conn;
            let read = s.read(bot).unwrap().version;
            for (r, c, v) in live {
                set(&s, p, r, c, v);
            }
            let said = match form {
                Form::WriteBase | Form::WriteBasePartial => {
                    let opts = if form == Form::WriteBasePartial {
                        GridWriteOptions::default().partial()
                    } else {
                        GridWriteOptions::default()
                    };
                    let r = s
                        .write_base_grid(bot, read, text.as_bytes(), &opts)
                        .unwrap();
                    let list = |c: &[k2_mpc_base::session::GridConflict]| {
                        c.iter().map(|x| x.to_string()).collect::<Vec<_>>()
                    };
                    match &r.outcome {
                        GridWrite::Applied(_) if r.skipped.is_empty() => "applied".to_owned(),
                        GridWrite::Applied(_) => format!("partial {:?}", list(&r.skipped)),
                        GridWrite::Conflicts(c) => format!("conflicts {:?}", list(c)),
                        o => format!("{o:?}"),
                    }
                }
                Form::Outside => {
                    f.save(&s, text);
                    format!("saved, overrides {:?}", f.overrides())
                }
            };
            let t = table_of(&s, p);
            let rows = rows_by_key(&t)
                .into_iter()
                .map(|(k, r)| (k.to_string(), r.join(",")))
                .collect();
            (rows, said)
        }

        fn rows_of(want: &[(&str, &str)]) -> Vec<(String, String)> {
            want.iter()
                .map(|(k, r)| ((*k).to_owned(), (*r).to_owned()))
                .collect()
        }

        /// A swap around a row that stays, nothing edited: each row only moved, so each keeps
        /// its key and the live edit stays with its row. (Pairing by place before moves gave
        /// each key the other's content.)
        #[test]
        fn swap_around_a_fixed_row_keeps_keys() {
            for form in FORMS {
                let (rows, said) = scenario(
                    "m0,m1\nz0,z1\nx0,x1\n",
                    &[("r1", "c1", "T")],
                    "x0,x1\nz0,z1\nm0,m1\n",
                    form,
                );
                println!("swap {form:?}: {rows:?} {said}");
                assert_eq!(
                    rows,
                    rows_of(&[("r3", "x0,x1"), ("r2", "z0,z1"), ("r1", "T,m1")]),
                    "{form:?}: {said}"
                );
            }
        }

        /// The same with a run of two between, so the anchors can't be anything else.
        #[test]
        fn swap_around_two_fixed_rows_keeps_keys() {
            for form in FORMS {
                let (rows, said) = scenario(
                    "m0,m1\ny0,y1\nz0,z1\nx0,x1\n",
                    &[("r1", "c1", "T")],
                    "x0,x1\ny0,y1\nz0,z1\nm0,m1\n",
                    form,
                );
                println!("swap2 {form:?}: {rows:?} {said}");
                assert_eq!(
                    rows,
                    rows_of(&[
                        ("r4", "x0,x1"),
                        ("r2", "y0,y1"),
                        ("r3", "z0,z1"),
                        ("r1", "T,m1")
                    ]),
                    "{form:?}: {said}"
                );
            }
        }

        /// K2 Engine's case: old `M a X b M y X`, new `X a M b X y M`: twins swapped around rows
        /// that stay, with live edits. It may cost identity; it must not make a row nobody wrote.
        #[test]
        fn seven_rows_with_twins_swapped_make_no_row_nobody_wrote() {
            let m = "m0,m1";
            let x = "x0,x1";
            let base = format!("{m}\na0,a1\n{x}\nb0,b1\n{m}\ny0,y1\n{x}\n");
            let text = format!("{x}\na0,a1\n{m}\nb0,b1\n{x}\ny0,y1\n{m}\n");
            let mut bad = Vec::new();
            for r in 1..=7 {
                for form in FORMS {
                    let at = format!("r{r}");
                    let (rows, said) = scenario(&base, &[(at.as_str(), "c1", "T")], &text, form);
                    println!("seven {at} {form:?}: {rows:?} {said}");
                    let own = |x: &str| {
                        x == "m0,m1" || x == "x0,x1" || x == "a0,a1" || x == "b0,b1" || x == "y0,y1"
                    };
                    let spliced: Vec<_> = rows
                        .iter()
                        .filter(|(_, r)| {
                            !own(r)
                                && !matches!(r.as_str(), "T,m1" | "T,x1" | "T,a1" | "T,b1" | "T,y1")
                        })
                        .collect();
                    // a live cell with another row's other cells can't be told apart from T on
                    // that row, so the row must be its own key's: checked by the base row
                    let keyed = rows
                        .iter()
                        .filter(|(_, r)| r.starts_with("T,"))
                        .all(|(k, v)| {
                            let want = match k.as_str() {
                                "r1" | "r5" => "T,m1",
                                "r2" => "T,a1",
                                "r3" | "r7" => "T,x1",
                                "r4" => "T,b1",
                                "r6" => "T,y1",
                                _ => "",
                            };
                            v == want
                        });
                    if !spliced.is_empty() || !keyed {
                        bad.push(format!("{at} {form:?}: {rows:?}"));
                    }
                }
            }
            assert!(bad.is_empty(), "{bad:?}");
        }

        /// A row with a duplicate in the swap: old `M Z X M`, new `X Z M M`, with a live edit
        /// on X. No key may show another row's cells with the live value.
        #[test]
        fn swap_with_a_duplicate_row_never_splices() {
            for form in FORMS {
                let (rows, said) = scenario(
                    "m0,m1\nz0,z1\nx0,x1\nm0,m1\n",
                    &[("r3", "c1", "T")],
                    "x0,x1\nz0,z1\nm0,m1\nm0,m1\n",
                    form,
                );
                println!("duplicate {form:?}: {rows:?} {said}");
                let x = rows
                    .iter()
                    .find(|(k, _)| k == "r3")
                    .map(|(_, r)| r.as_str());
                assert_eq!(x, Some("T,x1"), "{form:?}: {rows:?} {said}");
            }
        }

        /// The tie: two old rows hold what one new row holds, so no move pairs it, and it
        /// lands in a gap of one old row and one new row: old `M M Y Z A`, new `B Y Z M`, with
        /// a live edit on A. Nothing may give A's key M's cells beside the live value.
        #[test]
        fn exact_row_with_two_old_twins_in_an_equal_gap_never_splices() {
            let mut spliced = Vec::new();
            for form in FORMS {
                let (rows, said) = scenario(
                    "m0,m1\nm0,m1\ny0,y1\nz0,z1\na0,a1\n",
                    &[("r5", "c1", "T")],
                    "b0,b1\ny0,y1\nz0,z1\nm0,m1\n",
                    form,
                );
                println!("tie {form:?}: {rows:?} {said}");
                if rows.iter().any(|(_, r)| r == "T,m1") {
                    spliced.push(form);
                }
            }
            assert!(spliced.is_empty(), "spliced in {spliced:?}");
        }
    }
    /// K2 Engine's review of the row-level rule, reasoned and to be reproduced: columns,
    /// undo and where the note goes. Each states the ruled outcome.
    mod row_rule_review {
        use super::*;
        use k2_mpc_base::session::RosterEvent;
        use k2_mpc_base::tools::ToolOutput;

        fn rows(s: &Session, c: ConnId) -> Vec<String> {
            let t = table_of(s, c);
            let cols = t.live_cols();
            t.live_rows()
                .into_iter()
                .map(|r| {
                    let f: Vec<&str> = cols.iter().map(|c| t.cell(&r, c).unwrap_or("")).collect();
                    format!("{r}={}", f.join(","))
                })
                .collect()
        }

        /// The file's columns swapped by content: every row looks unchanged under the mapping,
        /// so the live cell stays with its column, in another field than the editor saw it.
        /// Ruled: a mapping by content isn't exact, so a live-edited row is whole.
        #[test]
        fn columns_mapped_by_content_make_a_live_edited_row_whole() {
            let (base, text) = ("p,q\nr,s\n", "q,p\ns,r\n");
            let mut wrong = Vec::new();
            for partial in [false, true] {
                let f = Fix::new();
                std::fs::write(f.file(), base).unwrap();
                let s = f.open();
                let p = f.join(&s, "p1");
                let bot = s
                    .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                    .unwrap()
                    .conn;
                let read = s.read(bot).unwrap().version;
                set(&s, p, "r1", "c1", "T");
                let opts = if partial {
                    GridWriteOptions::default().partial()
                } else {
                    GridWriteOptions::default()
                };
                let r = s
                    .write_base_grid(bot, read, text.as_bytes(), &opts)
                    .unwrap();
                let got = rows(&s, p);
                let said = format!("{:?} skipped {:?}", r.outcome, r.skipped);
                println!(
                    "columns partial={partial}: {got:?} {}",
                    &said[..said.len().min(300)]
                );
                let clean = matches!(r.outcome, GridWrite::Applied(_)) && r.skipped.is_empty();
                if clean {
                    wrong.push(format!("write --base partial={partial} clean: {got:?}"));
                }
            }
            let f = Fix::new();
            std::fs::write(f.file(), base).unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c1", "T");
            f.save(&s, text);
            let got = rows(&s, p);
            let notes = overwritten(&s.take_notes(p).unwrap());
            println!("columns outside: {got:?} notes {notes:?}");
            if !got.iter().any(|r| r.ends_with("=q,p")) || notes.is_empty() {
                wrong.push(format!("outside: {got:?}, {} note(s)", notes.len()));
            }
            assert!(wrong.is_empty(), "{wrong:?}");
        }

        /// A rewrite pairs A' with B's key (A' like B, B deleted); then someone edits that row
        /// at a current base, looking at A'; then the rewrite is undone. Ruled (S13): `All`
        /// conflicts; `--partial` skips the row and says so, the rest applied; never B's old cells
        /// beside an edit made on A'. Redo after that partial undo redoes only what it did.
        #[test]
        fn undo_of_a_rewrite_with_a_later_edit_on_the_row_conflicts() {
            let start = || {
                let f = Fix::new();
                std::fs::write(f.file(), "v0x0,v0x1\nv1x0,v1x1\n").unwrap();
                let s = f.open();
                let p = f.join(&s, "p1");
                let bot = s
                    .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                    .unwrap()
                    .conn;
                let read = s.read(bot).unwrap().version;
                let r = s
                    .write_base_grid(bot, read, b"v1x0,n1\n", &GridWriteOptions::default())
                    .unwrap();
                assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
                assert_eq!(
                    rows(&s, p),
                    ["r2=v1x0,n1"],
                    "paired with B's key by likeness"
                );
                set(&s, p, "r2", "c1", "E");
                (f, s, p, bot)
            };
            // all or nothing: a conflict, nothing applied
            let (_f, s, p, bot) = start();
            let u = s
                .undo_with(bot, &UndoRequest::steps(1).mode(UndoMode::All))
                .unwrap();
            let UndoOutcome::Conflicts { conflicts, .. } = &u else {
                panic!("{u:?}")
            };
            assert!(
                conflicts
                    .iter()
                    .all(|c| c.why.as_deref() == Some("rewritten_row")),
                "{conflicts:?}"
            );
            assert_eq!(rows(&s, p), ["r2=E,n1"], "nothing applied");
            // --partial: the rest applies, the row is reported as skipped
            let (_f, s, p, bot) = start();
            let u = s
                .undo_with(bot, &UndoRequest::steps(1).mode(UndoMode::Partial))
                .unwrap();
            let UndoOutcome::Applied { skipped, .. } = &u else {
                panic!("{u:?}")
            };
            assert!(
                !skipped.is_empty()
                    && skipped
                        .iter()
                        .all(|c| c.why.as_deref() == Some("rewritten_row")),
                "the skip is visible to the caller: {skipped:?}"
            );
            assert_eq!(rows(&s, p), ["r1=v0x0,v0x1", "r2=E,n1"], "never E,v1x1");
            // redo: only what the undo did: r1 deleted again, r2 as it is
            let r = s
                .redo_with(bot, &UndoRequest::steps(1).mode(UndoMode::All))
                .unwrap();
            assert!(matches!(r, UndoOutcome::Applied { .. }), "{r:?}");
            assert_eq!(rows(&s, p), ["r2=E,n1"]);
        }

        fn with_bot(f: &Fix, body: &str) -> (Arc<Session>, ConnId, ConnId) {
            std::fs::write(f.file(), body).unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            let bot = s
                .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                .unwrap()
                .conn;
            (s, p, bot)
        }

        fn text_applies(s: &Session, bot: ConnId, base: u64, text: &str) {
            let r = s
                .write_base_grid(bot, base, text.as_bytes(), &GridWriteOptions::default())
                .unwrap();
            assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
        }

        fn is_rewritten(r: Result<u64, SessionError>) -> bool {
            matches!(r, Err(SessionError::FacetConflict(c)) if c.iter().all(|c| c.rewritten_row))
        }

        /// A keyed write whose base predates a text's non-exact pairing of that row is refused,
        /// even on a cell the text didn't change; at the current version it applies.
        #[test]
        fn a_stale_keyed_write_after_a_both_edited_swap_is_refused() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "a,1\nc,2\nz,9\n");
            let old = s.read(bot).unwrap().version;
            text_applies(&s, bot, old, "c,X\na,Y\nz,9\n");
            let now = s.read(bot).unwrap().version;
            let stale = EditOptions::new().base(old);
            assert!(
                is_rewritten(op(&s, bot, &stale, |t| t.set_cell(&k("r1"), &k("c1"), "x"))),
                "a cell the text didn't change"
            );
            assert!(is_rewritten(op(&s, bot, &stale, |t| t.delete_row(&k("r2")))));
            assert!(
                op(&s, bot, &stale, |t| t.set_cell(&k("r3"), &k("c1"), "ok")).is_ok(),
                "a row the text left exact keeps the per-cell rule"
            );
            let current = EditOptions::new().base(s.read(bot).unwrap().version);
            assert!(op(&s, bot, &current, |t| t.set_cell(&k("r1"), &k("c1"), "x")).is_ok());
            let _ = (p, now);
        }

        /// (a) A non-exact column mapping marks every paired row, even one whose values are
        /// unchanged. Flip (K2 Cores' option 1): a header-less swap at the same width is no longer
        /// non-exact (columns pair by position), so the swap is reached through a width change
        /// (content pairing finds it beside a new column); at the same width, r1 is exact and
        /// unmarked.
        #[test]
        fn a_column_swap_marks_a_row_that_renders_the_same() {
            {
                let f = Fix::new();
                let (s, _p, bot) = with_bot(&f, "a,a\nb,c\n");
                let old = s.read(bot).unwrap().version;
                text_applies(&s, bot, old, "a,a,\nc,b,M\n");
                let stale = EditOptions::new().base(old);
                assert!(is_rewritten(op(&s, bot, &stale, |t| t.set_cell(
                    &k("r1"),
                    &k("c1"),
                    "x"
                ))));
            }
            // at the same width: by position, exact; r1 unchanged, so not marked
            let f = Fix::new();
            let (s, _p, bot) = with_bot(&f, "a,a\nb,c\n");
            let old = s.read(bot).unwrap().version;
            text_applies(&s, bot, old, "a,a\nc,b\n");
            let stale = EditOptions::new().base(old);
            assert!(op(&s, bot, &stale, |t| t.set_cell(&k("r1"), &k("c1"), "x")).is_ok());
        }

        /// (b) An undo that changes a marked row marks it at its own version: a client that
        /// read the rewrite's text can't write to the row after the undo either.
        #[test]
        fn an_undo_of_a_rewrite_marks_the_row_again() {
            let f = Fix::new();
            let (s, _p, bot) = with_bot(&f, "v0x0,v0x1\nv1x0,v1x1\n");
            let old = s.read(bot).unwrap().version;
            text_applies(&s, bot, old, "v1x0,n1\n");
            let after_text = s.read(bot).unwrap().version;
            assert!(matches!(
                s.undo_with(bot, &UndoRequest::steps(1).mode(UndoMode::All))
                    .unwrap(),
                UndoOutcome::Applied { .. }
            ));
            let stale = EditOptions::new().base(after_text);
            assert!(is_rewritten(op(&s, bot, &stale, |t| t.set_cell(
                &k("r2"),
                &k("c1"),
                "x"
            ))));
        }

        /// (c) Every key a verb names: a re-paired anchor refuses the move too.
        #[test]
        fn a_re_paired_anchor_refuses_a_move_after_it() {
            let f = Fix::new();
            let (s, _p, bot) = with_bot(&f, "a,1\nb,2\nc,3\n");
            let old = s.read(bot).unwrap().version;
            text_applies(&s, bot, old, "a,1\nb,X\nc,3\n");
            let stale = EditOptions::new().base(old);
            assert!(is_rewritten(op(&s, bot, &stale, |t| t
                .move_row(&k("r1"), Anchor::After(k("r2"))))));
        }

        /// A later exact pair doesn't clear the mark; the mark persists across a restart.
        #[test]
        fn a_mark_survives_a_later_exact_pair_and_a_restart() {
            let mut f = Fix::new();
            let (s, _p, bot) = with_bot(&f, "a,1\nb,2\nc,3\n");
            let old = s.read(bot).unwrap().version;
            text_applies(&s, bot, old, "a,1\nb,X\nc,3\n");
            let v2 = s.read(bot).unwrap().version;
            text_applies(&s, bot, v2, "a,Y\nb,X\nc,3\n");
            let stale = EditOptions::new().base(old);
            assert!(is_rewritten(op(&s, bot, &stale, |t| t.set_cell(
                &k("r2"),
                &k("c1"),
                "x"
            ))));
            drop(s);
            f.restart();
            let s = f.open();
            let bot = s
                .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                .unwrap()
                .conn;
            assert!(is_rewritten(op(&s, bot, &stale, |t| t.set_cell(
                &k("r2"),
                &k("c1"),
                "x"
            ))));
        }

        /// A person's edit with a seen version is guarded; without one it lands at current (the
        /// contract's stated limit).
        #[test]
        fn a_seen_version_guards_a_persons_edit() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "a,1\nb,2\n");
            let old = s.read(p).unwrap().version;
            text_applies(&s, bot, old, "a,1\nb,X\n");
            let seen = EditOptions::new().seen(old);
            assert!(is_rewritten(op(&s, p, &seen, |t| t.set_cell(
                &k("r2"),
                &k("c1"),
                "x"
            ))));
            let fresh = EditOptions::new().seen(s.read(p).unwrap().version);
            assert!(op(&s, p, &fresh, |t| t.set_cell(&k("r2"), &k("c1"), "x")).is_ok());
            assert!(op(&s, p, &EditOptions::new(), |t| t.set_cell(
                &k("r2"),
                &k("c1"),
                "y"
            ))
            .is_ok());
        }

        /// Call 1: a live change between empty and absent is no live edit for the both-changed
        /// test, and if the file left the row alone the live state stays as it is.
        #[test]
        fn empty_and_absent_are_no_live_edit_and_stay_where_the_file_left_the_row() {
            let unset = |s: &Session, p: ConnId| {
                s.edit_ops(
                    p,
                    vec![Edit::Map {
                        root: CELLS.into(),
                        ops: vec![MapOp::Delete {
                            key: table::cell_key(&k("r1"), &k("c1")),
                        }],
                    }],
                    &EditOptions::new(),
                )
                .unwrap();
            };
            // the file changed the row: no both-changed conflict, no note
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, ",1\nb,2\n");
            let old = s.read(bot).unwrap().version;
            unset(&s, p);
            assert_eq!(table_of(&s, p).cell(&k("r1"), &k("c1")), None);
            text_applies(&s, bot, old, ",X\nb,2\n");
            assert!(overwritten(&s.take_notes(p).unwrap()).is_empty());
            // the file left the row alone: the absent cell stays absent
            let f = Fix::new();
            let (s, p, _bot) = with_bot(&f, ",1\nb,2\n");
            unset(&s, p);
            f.save(&s, ",1\nb,X\n");
            assert_eq!(table_of(&s, p).cell(&k("r1"), &k("c1")), None);
            assert!(overwritten(&s.take_notes(p).unwrap()).is_empty());
        }

        /// A catch-all `{:?}` log never writes document text: the note types redact their values.
        #[test]
        fn debug_never_prints_a_notes_values() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            let rx = s.subscribe_roster(p).unwrap();
            set(&s, p, "r2", "c1", "SECRETLIVE");
            set(&s, p, "r2", "c2", "SECRETLIVE2");
            f.save(&s, "a,b\nSECRETFILE,d\n");
            f.advance(1_000);
            s.flush_roster().unwrap();
            let pushed: Vec<RosterEvent> = rx.try_iter().collect();
            let event = pushed
                .iter()
                .find(|e| matches!(e, RosterEvent::Overwritten { .. }))
                .expect("pushed");
            let n = s.take_notes(p).unwrap();
            let text = format!(
                "{event:?} {pushed:?} {n:?} {:?} {:?}",
                n.notes, n.notes[0].overwrite
            );
            assert!(!text.contains("SECRET"), "{text}");
            // the audit events carry no values either
            let audit = format!("{:?}", f.overrides());
            assert!(!audit.contains("SECRET"), "{audit}");
            // Display does: it is for the person
            assert!(n.notes[0].to_string().contains("SECRETLIVE"));
        }

        /// A value is capped in the note (the version has it whole).
        #[test]
        fn a_large_value_is_cut_and_the_version_holds_it() {
            let big = "é".repeat(2_000);
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r2", "c2", &big);
            f.save(&s, "a,b\nc,FILE\n");
            let ow = overwritten(&s.take_notes(p).unwrap());
            assert!(ow[0].cut);
            assert!(ow[0].yours.len() <= k2_mpc_base::session::OVERWRITE_VALUE_MAX);
            let vid = ow[0].version.unwrap();
            assert_eq!(
                version_holds(&s, p, vid, "r2", "c2").as_deref(),
                Some(big.as_str())
            );
        }

        /// Recovery from only what the note says: the row and column, and the version to read.
        #[test]
        fn a_lost_edit_comes_back_from_the_note_alone() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r2", "c1", "LOST");
            f.save(&s, "a,b\nc,FILE\n");
            let n = overwritten(&s.take_notes(p).unwrap()).remove(0);
            // nothing else is used: the note's region and its version
            let (r, c) = n.region.split_once('/').unwrap();
            let got = version_holds(&s, p, n.version.unwrap(), r, c).unwrap();
            assert_eq!(got, "LOST");
            set(&s, p, r, c, &got);
            assert_eq!(table_of(&s, p).cell(&k(r), &k(c)), Some("LOST"));
        }

        /// Waiting notes go in a new subscriber's snapshot, values and all.
        #[test]
        fn a_snapshot_carries_the_waiting_notes() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r2", "c2", "LIVE");
            f.save(&s, "a,b\nc,FILE\n");
            let rx = s.subscribe_roster(p).unwrap();
            let got: Vec<RosterEvent> = rx.try_iter().collect();
            let RosterEvent::Snapshot { overwritten: o, .. } = &got[0] else {
                panic!("{got:?}")
            };
            assert_eq!(o.len(), 1);
            assert_eq!(o[0].yours, "\"LIVE\"");
            assert!(o[0].version.is_some());
            // and it is still there for the pull
            assert_eq!(overwritten(&s.take_notes(p).unwrap()).len(), 1);
        }

        /// Someone who has lost read access gets nothing, not even a count: no event, no
        /// snapshot entry, no note. The note stays saved; with read back, it arrives.
        #[test]
        fn lost_read_access_gets_nothing_until_it_is_back() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            let rx = s.subscribe_roster(p).unwrap();
            let _ = rx.try_iter().count();
            set(&s, p, "r2", "c1", "SECRETLIVE");
            f.save(&s, "a,b\nSECRETFILE,d\n");
            let mut no_read = Rights::full().with_owner(true);
            no_read.read = false;
            f.host.rights.lock().unwrap().insert("p1".into(), no_read);
            f.advance(1_000);
            s.flush_roster().unwrap();
            let got: Vec<RosterEvent> = rx.try_iter().collect();
            assert!(
                !got.iter()
                    .any(|e| matches!(e, RosterEvent::Overwritten { .. })),
                "{got:?}"
            );
            assert!(s.take_notes(p).is_err(), "no read, no notes");
            // read back: a new connection finds it
            f.host
                .rights
                .lock()
                .unwrap()
                .insert("p1".into(), Rights::full().with_owner(true));
            let p = f.join(&s, "p1");
            let ow = overwritten(&s.take_notes(p).unwrap());
            assert_eq!(ow.len(), 1);
            assert_eq!(ow[0].yours, "\"SECRETLIVE\"");
        }

        // ---- K2 Engine's kit diff on the row rule, each as a named case ----

        /// A row the live side edited and then deleted, and the file changed: both changed. The
        /// file's row comes back whole, with a note carrying the live value.
        #[test]
        fn edit_then_delete_then_a_save_returns_the_files_row_whole() {
            let f = Fix::new();
            std::fs::write(f.file(), "\nz,z\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c2", "a");
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
            f.save(&s, ",\nz,z\n");
            assert_eq!(rows(&s, p), ["r1=,", "r2=z,z"], "not ,a");
            let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            assert!(
                n.iter().any(|c| c.2 == "\"a\""),
                "the note has the live value: {n:?}"
            );
        }

        /// The same at the same width, header-less. The column-swap point now lives in the
        /// width-changing variant (`edit_then_delete_under_a_width_changing_column_swap_lands_nowhere_wrong`);
        /// this case pins §2.9.1 (flip, K2 Cores): columns pair by position, so the file's blank
        /// line is exact against r2 and r2 stays deleted by the live side (its own `""` hidden);
        /// the file's `a,` shares no field in place with r1's `,a`, so it is a new row, and r1 is
        /// deleted by the file with its cells hidden. No note: the live side never touched r1.
        #[test]
        fn edit_then_delete_under_a_column_swap_lands_nowhere_wrong() {
            let f = Fix::new();
            std::fs::write(f.file(), ",a\n\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r2", "c1", "");
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
            f.save(&s, "\na,\n");
            let got = rows(&s, p);
            let t = table_of(&s, p);
            let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            println!("{got:?} cells {:?} notes {n:?}", t.cells);
            // the file's bytes, but for the blank line the live side deleted
            let text = String::from_utf8(s.read(p).unwrap().bytes).unwrap();
            assert_eq!(text, "a,\n");
            assert!(!t.rows.is_live(&k("r2")), "r2 stays deleted");
            assert_eq!(
                t.cell(&k("r2"), &k("c1")),
                Some(""),
                "the live side's own value, hidden"
            );
            assert!(!t.rows.is_live(&k("r1")), "r1 deleted by the file");
            assert_eq!(t.cell(&k("r1"), &k("c1")), Some(""));
            assert_eq!(
                t.cell(&k("r1"), &k("c2")),
                Some("a"),
                "r1's cells hidden in place"
            );
            let live = t.live_rows();
            assert_eq!(live.len(), 1, "{live:?}");
            let r3 = &live[0];
            assert!(!matches!(r3.as_str(), "r1" | "r2"), "a new row: {r3}");
            assert_eq!(t.cell(r3, &k("c1")), Some("a"));
            assert_eq!(t.cell(r3, &k("c2")), None, "c2 absent, rendered empty");
            assert_eq!(got, [format!("{r3}=a,")]);
            assert!(n.is_empty(), "no note: {n:?}");
        }

        /// The swap kept under test (K2 Cores, with flip 3): the file swaps the columns while adding
        /// one (a width change, so content pairing finds the swap). The live edit-then-delete of
        /// r2 is handled (the file's row comes back, noted), the text is the file's, and `a` stays
        /// in c2, wherever c2 now sits.
        #[test]
        fn edit_then_delete_under_a_width_changing_column_swap_lands_nowhere_wrong() {
            let f = Fix::new();
            std::fs::write(f.file(), ",a\n\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r2", "c1", "");
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
            f.save(&s, "\na,,N\n");
            let t = table_of(&s, p);
            let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            let text = String::from_utf8(s.read(p).unwrap().bytes).unwrap();
            println!(
                "rows {:?} cols {:?} cells {:?} text {text:?} notes {n:?}",
                rows(&s, p),
                t.live_cols(),
                t.cells
            );
            assert_eq!(text, "\na,,N\n", "the file's rows, whole");
            assert_eq!(&t.live_cols()[..2], [k("c2"), k("c1")], "the swap found");
            assert!(t.rows.is_live(&k("r2")), "the file's row comes back");
            assert!(
                n.iter().any(|c| c.0 == "row r2" && c.1 == "deleted_row"),
                "{n:?}"
            );
            let a: Vec<_> = t
                .cells
                .iter()
                .filter(|(_, v)| v.as_str() == "a")
                .map(|(at, _)| at.1.clone())
                .collect();
            assert_eq!(a, [k("c2")], "a only in c2");
        }

        /// A live delete where the file changed the row only through a column: no exception.
        /// `write --base` conflicts; an outside save returns the row, with a note.
        #[test]
        fn a_live_delete_against_a_column_only_change() {
            // a column swap
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "a,b\nc,d\n");
            let old = s.read(bot).unwrap().version;
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
            let r = s
                .write_base_grid(bot, old, b"b,a\nd,c\n", &GridWriteOptions::default())
                .unwrap();
            assert!(matches!(r.outcome, GridWrite::Conflicts(_)), "{r:?}");
            let f = Fix::new();
            let (s, p, _bot) = with_bot(&f, "a,b\nc,d\n");
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
            f.save(&s, "b,a\nd,c\n");
            let t = String::from_utf8(s.read(p).unwrap().bytes).unwrap();
            assert_eq!(t, "b,a\nd,c\n", "the row returns");
            assert!(
                !overwritten(&s.take_notes(p).unwrap()).is_empty(),
                "with a note"
            );
            // a column delete
            let f = Fix::new();
            let (s, p, _bot) = with_bot(&f, "x,y\nz,w\n");
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
            f.save(&s, "y\nw\n");
            let t = String::from_utf8(s.read(p).unwrap().bytes).unwrap();
            assert_eq!(t, "y\nw\n", "the row returns");
            assert!(
                !overwritten(&s.take_notes(p).unwrap()).is_empty(),
                "with a note"
            );
        }

        /// Both moved one row through a text: the text's place, no conflict, on either channel.
        #[test]
        fn both_moved_the_texts_place_with_no_conflict() {
            let body = "a,1\nb,2\nc,3\n";
            let text = "b,2\nc,3\na,1\n";
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, body);
            let old = s.read(bot).unwrap().version;
            op(&s, p, &EditOptions::new(), |t| {
                t.move_row(&k("r1"), Anchor::After(k("r2")))
            })
            .unwrap();
            text_applies(&s, bot, old, text);
            assert_eq!(table_of(&s, p).live_rows(), [k("r2"), k("r3"), k("r1")]);
            let f = Fix::new();
            let (s, p, _bot) = with_bot(&f, body);
            op(&s, p, &EditOptions::new(), |t| {
                t.move_row(&k("r1"), Anchor::After(k("r2")))
            })
            .unwrap();
            f.save(&s, text);
            assert_eq!(table_of(&s, p).live_rows(), [k("r2"), k("r3"), k("r1")]);
            assert!(overwritten(&s.take_notes(p).unwrap()).is_empty(), "no note");
            // move + set on an exact row: the set stays, the move adds no conflict
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, body);
            let old = s.read(bot).unwrap().version;
            op(&s, p, &EditOptions::new(), |t| {
                t.move_row(&k("r1"), Anchor::After(k("r2")))
            })
            .unwrap();
            set(&s, p, "r1", "c2", "LIVE");
            text_applies(&s, bot, old, text);
            assert_eq!(
                rows(&s, p),
                ["r2=b,2", "r3=c,3", "r1=a,LIVE"],
                "exact: the live cell stays"
            );
        }

        /// The file moved an unchanged row the live side deleted: it stays deleted, no conflict,
        /// no note, on both channels.
        #[test]
        fn a_text_move_of_an_exact_row_doesnt_bring_it_back() {
            let body = "a,1\nb,2\nc,3\n";
            let text = "b,2\nc,3\na,1\n";
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, body);
            let old = s.read(bot).unwrap().version;
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
            text_applies(&s, bot, old, text);
            assert_eq!(rows(&s, p), ["r2=b,2", "r3=c,3"]);
            let f = Fix::new();
            let (s, p, _bot) = with_bot(&f, body);
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
            f.save(&s, text);
            assert_eq!(rows(&s, p), ["r2=b,2", "r3=c,3"]);
            assert!(overwritten(&s.take_notes(p).unwrap()).is_empty());
        }

        /// A file's delete of a row the live side only moved stands: no conflict, no note.
        #[test]
        fn a_text_delete_stands_over_a_live_move() {
            let body = "a,1\nb,2\nc,3\n";
            let text = "b,2\nc,3\n";
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, body);
            let old = s.read(bot).unwrap().version;
            op(&s, p, &EditOptions::new(), |t| {
                t.move_row(&k("r1"), Anchor::End)
            })
            .unwrap();
            text_applies(&s, bot, old, text);
            assert_eq!(rows(&s, p), ["r2=b,2", "r3=c,3"]);
            let f = Fix::new();
            let (s, p, _bot) = with_bot(&f, body);
            op(&s, p, &EditOptions::new(), |t| {
                t.move_row(&k("r1"), Anchor::End)
            })
            .unwrap();
            f.save(&s, text);
            assert_eq!(rows(&s, p), ["r2=b,2", "r3=c,3"]);
            assert!(overwritten(&s.take_notes(p).unwrap()).is_empty());
            assert_eq!(f.overrides().len(), 1, "on the audit record");
        }

        /// (e): "equal to the rewrite" is per row. One matching cell doesn't make the rows equal.
        #[test]
        fn an_edit_equal_to_the_rewrite_is_per_row() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "a,b\nz,z\n");
            let old = s.read(bot).unwrap().version;
            // the same cell as the text's, but the text changed another one too
            set(&s, p, "r1", "c1", "A");
            let r = s
                .write_base_grid(bot, old, b"A,B\nz,z\n", &GridWriteOptions::default())
                .unwrap();
            assert!(matches!(r.outcome, GridWrite::Conflicts(_)), "{r:?}");
            // the whole row equal: no conflict
            set(&s, p, "r1", "c2", "B");
            text_applies(&s, bot, old, "A,B\nz,z\n");
            assert_eq!(rows(&s, p)[0], "r1=A,B");
        }

        // ---- whose value a whole rewrite overrides ----

        /// A's value overridden by B's restore gives A a note; B's own gives B none; a value
        /// nobody wrote since the import has nobody to tell, and stays on the audit record.
        #[test]
        fn a_restore_notes_others_values_not_ones_own() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let (pa, pb) = (f.join(&s, "p1"), f.join(&s, "p2"));
            // r2/c1 is changed before the version and put back after it: live holds the
            // import's value, the version another
            set(&s, pb, "r2", "c1", "THEN");
            let v = s.name_version(pb, "before", None).unwrap();
            assert!(matches!(
                s.undo_with(pb, &UndoRequest::steps(1).mode(UndoMode::All))
                    .unwrap(),
                UndoOutcome::Applied { .. }
            ));
            set(&s, pa, "r1", "c1", "A");
            set(&s, pb, "r1", "c2", "B");
            s.restore(pb, v).unwrap();
            assert_eq!(rows(&s, pa), ["r1=a,b", "r2=THEN,d"]);
            let a = cells_of(&overwritten(&s.take_notes(pa).unwrap()));
            assert_eq!(a.len(), 1, "{a:?}");
            assert_eq!(a[0].0, "r1/c1");
            assert!(
                overwritten(&s.take_notes(pb).unwrap()).is_empty(),
                "B's own"
            );
            let audit = f.overrides();
            println!("restore audit: {audit:?}");
            assert!(audit
                .iter()
                .any(|o| o.1 == "r1/c1" && o.2.as_deref() == Some("p1")));
            assert!(
                !audit.iter().any(|o| o.1 == "r1/c2"),
                "B's own: no override"
            );
            assert!(
                audit
                    .iter()
                    .any(|o| o.1 == "r2/c1" && o.2.as_deref() == Some("kit:import")),
                "the import's value: no note, on the audit record: {audit:?}"
            );
        }

        /// The same for a stale live write.
        #[test]
        fn a_stale_write_notes_others_values_not_ones_own() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let (pa, pb) = (f.join(&s, "p1"), f.join(&s, "p2"));
            let seen = s.read(pb).unwrap().version;
            set(&s, pa, "r1", "c1", "A");
            set(&s, pb, "r2", "c1", "B");
            s.write_live_grid(pb, seen, b"a,X\nc,Y\n", &GridWriteOptions::default())
                .unwrap();
            assert_eq!(rows(&s, pa), ["r1=a,X", "r2=c,Y"]);
            let a = cells_of(&overwritten(&s.take_notes(pa).unwrap()));
            assert_eq!(a.len(), 1, "{a:?}");
            assert_eq!(a[0].0, "r1/c1");
            assert!(
                overwritten(&s.take_notes(pb).unwrap()).is_empty(),
                "B's own"
            );
        }

        /// The same for a revert that rewrites a re-paired row whole.
        #[test]
        fn a_revert_notes_others_values_not_ones_own() {
            let f = Fix::new();
            std::fs::write(f.file(), "v0,v1\nw0,w1\n").unwrap();
            let s = f.open();
            let pa = f.join(&s, "p1");
            let bot = s
                .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                .unwrap()
                .conn;
            let read = s.read(bot).unwrap().version;
            let r = s
                .write_base_grid(bot, read, b"v0,X\nw0,Y\n", &GridWriteOptions::default())
                .unwrap();
            let GridWrite::Applied(res) = &r.outcome else {
                panic!("{r:?}")
            };
            let change = res.version;
            set(&s, pa, "r1", "c1", "A");
            let cur = EditOptions::new().base(s.read(bot).unwrap().version);
            op(&s, bot, &cur, |t| t.set_cell(&k("r2"), &k("c1"), "B")).unwrap();
            let out = s.revert(bot, change, false).unwrap();
            assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
            assert_eq!(rows(&s, pa), ["r1=v0,v1", "r2=w0,w1"], "both rows whole");
            let a = cells_of(&overwritten(&s.take_notes(pa).unwrap()));
            assert_eq!(a.len(), 1, "{a:?}");
            assert_eq!(a[0].0, "r1/c1");
            let vid = a[0].4.expect("a version");
            assert_eq!(version_holds(&s, pa, vid, "r1", "c1").as_deref(), Some("A"));
            assert!(
                overwritten(&s.take_notes(bot).unwrap()).is_empty(),
                "the reverter's own"
            );
        }

        /// A stale write over a value the import holds: nobody to tell, on the audit record.
        #[test]
        fn a_stale_write_over_the_imports_value_keeps_its_audit_event() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let w = f.join(&s, "p2");
            // p2's view holds THEN; the undo puts the import's value back
            set(&s, w, "r2", "c1", "THEN");
            let seen = s.read(w).unwrap().version;
            assert!(matches!(
                s.undo_with(w, &UndoRequest::steps(1).mode(UndoMode::All))
                    .unwrap(),
                UndoOutcome::Applied { .. }
            ));
            s.write_live_grid(w, seen, b"a,b\nTHEN,X\n", &GridWriteOptions::default())
                .unwrap();
            assert_eq!(rows(&s, w), ["r1=a,b", "r2=THEN,X"]);
            assert!(overwritten(&s.take_notes(w).unwrap()).is_empty());
            let audit = f.overrides();
            assert!(
                audit
                    .iter()
                    .any(|o| o.1 == "r2/c1" && o.2.as_deref() == Some("kit:import")),
                "{audit:?}"
            );
        }

        /// A revert can't meet a value the import holds with another value before the text: the
        /// only way there is an undo of an earlier write on the row, and S13 refuses that once a
        /// text has re-paired the row. The revert then rewrites the row whole: the writer of the
        /// later cell gets the note, the reverter's own value none.
        #[test]
        fn a_revert_cant_meet_the_imports_value_here() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b,c\nz,z,z\n").unwrap();
            let s = f.open();
            let (pa, pb) = (f.join(&s, "p1"), f.join(&s, "p2"));
            let bot = s
                .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                .unwrap()
                .conn;
            set(&s, pa, "r1", "c1", "X");
            let read = s.read(bot).unwrap().version;
            let r = s
                .write_base_grid(bot, read, b"X,B,c\nz,z,z\n", &GridWriteOptions::default())
                .unwrap();
            let GridWrite::Applied(res) = &r.outcome else {
                panic!("{r:?}")
            };
            let change = res.version;
            set(&s, pb, "r1", "c3", "L");
            // the earlier write's undo: refused, a text re-paired the row since
            let u = s
                .undo_with(pa, &UndoRequest::steps(1).mode(UndoMode::All))
                .unwrap();
            assert!(matches!(u, UndoOutcome::Conflicts { .. }), "{u:?}");
            assert!(matches!(
                s.revert(bot, change, false).unwrap(),
                UndoOutcome::Applied { .. }
            ));
            assert_eq!(
                rows(&s, pa)[0],
                "r1=X,b,c",
                "the row as it was before the text"
            );
            let b = cells_of(&overwritten(&s.take_notes(pb).unwrap()));
            assert_eq!(b.len(), 1, "{b:?}");
            assert_eq!(b[0].0, "r1/c3");
            assert!(overwritten(&s.take_notes(pa).unwrap()).is_empty());
            assert!(!f
                .overrides()
                .iter()
                .any(|o| o.2.as_deref() == Some("kit:import")));
        }

        /// An undo of a text that remapped the columns bumps the remap version: a write at the
        /// swap's version is refused after it too.
        #[test]
        fn an_undo_of_a_remap_bumps_the_remap_version() {
            let f = Fix::new();
            let (s, _p, bot) = with_bot(&f, "a,b\n");
            let old = s.read(bot).unwrap().version;
            text_applies(&s, bot, old, "b,a\n");
            let at_swap = s.read(bot).unwrap().version;
            let u = s
                .undo_with(bot, &UndoRequest::steps(1).mode(UndoMode::All))
                .unwrap();
            assert!(matches!(u, UndoOutcome::Applied { .. }), "{u:?}");
            let stale = EditOptions::new().base(at_swap);
            assert!(is_rewritten(op(&s, bot, &stale, |t| t.set_cell(
                &k("r1"),
                &k("c1"),
                "x"
            ))));
        }

        /// A deleted row has no mark; the doc-level remap version covers a stale write to it. Flip
        /// (K2 Cores' option 1): a header-less swap at the same width is value rewrites, not a
        /// remap, so the remap is reached through a width change; at the same width the stale
        /// write lands hidden in the deleted row and is reported to its writer.
        #[test]
        fn a_remap_refuses_a_stale_write_to_a_deleted_row() {
            // (by key, as a stale client sends it: the table's own verb refuses a deleted row)
            let write = || {
                vec![Edit::Map {
                    root: CELLS.into(),
                    ops: vec![MapOp::Set {
                        key: table::cell_key(&k("r1"), &k("c1")),
                        value: Value::str("x"),
                    }],
                }]
            };
            for (second, remap) in [("d,c,N\n", true), ("d,c\n", false)] {
                let f = Fix::new();
                let (s, _p, bot) = with_bot(&f, "a,b\nc,d\n");
                let old = s.read(bot).unwrap().version;
                text_applies(&s, bot, old, "c,d\n");
                let v1 = s.read(bot).unwrap().version;
                text_applies(&s, bot, v1, second);
                let stale = EditOptions::new().base(old);
                let r = s.edit_ops(bot, write(), &stale);
                println!("second text {second:?}: {r:?}");
                if remap {
                    assert!(is_rewritten(r.map(|r| r.version)), "{second:?}");
                } else {
                    let r = r.unwrap();
                    assert_eq!(r.hidden.len(), 1, "{r:?}");
                    let h = &r.hidden[0];
                    assert_eq!((h.row.as_str(), h.col.as_str()), ("r1", "c1"));
                    assert!(h.row_hidden, "into a deleted row: {h:?}");
                    assert_eq!(h.deleted_by.as_deref(), Some("bot"), "{h:?}");
                }
            }
        }

        // ---- K2 Engine's kit diff at 876489e: its literal shapes ----

        /// Base [blank], file `,a`; live sets c2 := a, then deletes the row. An outside save:
        /// the row returns, with a note to the deleter. (A second row `,` gives the table its two
        /// columns: a file of one blank line has none.)
        #[test]
        fn engine_edit_equal_then_delete_then_a_save_is_noted() {
            let f = Fix::new();
            std::fs::write(f.file(), "\n,\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c2", "a");
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
            f.save(&s, ",a\n,\n");
            let t = String::from_utf8(s.read(p).unwrap().bytes).unwrap();
            assert_eq!(t, ",a\n,\n", "the row returns");
            let n = overwritten(&s.take_notes(p).unwrap());
            println!("notes {n:?}, audit {:?}", f.overrides());
            assert!(!n.is_empty(), "with a note");
        }

        /// The same through `write --base`: a conflict `deleted_row`.
        #[test]
        fn engine_edit_equal_then_delete_then_write_base_conflicts() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "\n,\n");
            let base = s.read(bot).unwrap().version;
            set(&s, p, "r1", "c2", "a");
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
            let r = s
                .write_base_grid(bot, base, b",a\n,\n", &GridWriteOptions::default())
                .unwrap();
            let GridWrite::Conflicts(c) = &r.outcome else {
                panic!("{r:?}")
            };
            assert!(
                c.iter().any(|c| c.conflict.rule.as_str() == "deleted_row"),
                "{c:?}"
            );
        }

        /// K2 Cores' final ruling on item 3: base [blank, `,,`], file [blank], the columns deleted;
        /// live on r1 (the row the file kept) sets c1 := `""`, then deletes it. A column delete is
        /// the file changing the row, so the live delete meets a changed row: it returns, noted.
        #[test]
        fn engine_column_delete_over_the_kept_row_edited_then_deleted_returns_it() {
            let f = Fix::new();
            std::fs::write(f.file(), "\n,,\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c1", "");
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
            f.save(&s, "\n");
            let t = table_of(&s, p);
            println!(
                "rows {:?} notes {:?} audit {:?}",
                t.live_rows(),
                overwritten(&s.take_notes(p).unwrap()),
                f.overrides()
            );
            assert!(t.rows.is_live(&k("r1")), "the row returns");
            assert!(
                f.overrides()
                    .iter()
                    .any(|o| o.0 == "deleted_row" && o.1.contains("r1")),
                "with a note: {:?}",
                f.overrides()
            );
        }

        /// The r2 shape: the file deleted r2 too, so it stays deleted (deleted by both).
        /// Base [blank, `,,`], file [blank]: the columns go; live sets r2.c1 := a, then deletes r2.
        /// With every column gone the rows can't be told apart, so pairing picks by place: the
        /// blank line is r1, r2 is deleted by both sides and stays deleted. The live value gets
        /// one `edited_col` note, and comes back from what the note says.
        #[test]
        fn engine_column_delete_over_an_edited_then_deleted_row() {
            let f = Fix::new();
            std::fs::write(f.file(), "\n,,\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r2", "c1", "a");
            op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
            f.save(&s, "\n");
            let t = table_of(&s, p);
            assert_eq!(t.live_rows(), [k("r1")], "the blank line pairs with r1");
            assert!(!t.rows.is_live(&k("r2")), "r2 stays deleted");
            let n = overwritten(&s.take_notes(p).unwrap());
            assert_eq!(n.len(), 1, "{n:?}");
            let o = &n[0];
            assert_eq!(o.rule, "edited_col");
            assert_eq!(o.region, "col c1");
            assert_eq!(o.col, Some(1));
            assert_eq!(o.yours, "\"a\"");
            let vid = o.version.expect("a version");
            assert_eq!(version_holds(&s, p, vid, "r2", "c1").as_deref(), Some("a"));
            let audit = f.overrides();
            assert!(
                audit.iter().any(|x| x.0 == "edited_col"
                    && x.1 == "col c1"
                    && x.2.as_deref() == Some("p1")),
                "the note is p1's: {audit:?}"
            );
            assert!(overwritten(&s.take_notes(f.join(&s, "p2")).unwrap()).is_empty());
            // back from what the note says: the version holds the value; the column and the row
            // it names are restored by key
            op(&s, p, &EditOptions::new(), |t| t.restore_col(&k("c1"))).unwrap();
            op(&s, p, &EditOptions::new(), |t| t.restore_row(&k("r2"))).unwrap();
            assert_eq!(table_of(&s, p).cell(&k("r2"), &k("c1")), Some("a"));
        }

        /// (f) Base [`,,`, `,a`], file [`,a`, `a,a`], the columns swapped; live c1 := a on the
        /// first row: the result is the file's own row, so it is clean.
        #[test]
        fn engine_an_equal_row_under_a_remap_is_clean() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, ",,\n,a\n");
            let base = s.read(bot).unwrap().version;
            set(&s, p, "r1", "c1", "a");
            let r = s
                .write_base_grid(bot, base, b",a\na,a\n", &GridWriteOptions::default())
                .unwrap();
            println!("{r:?} -> {:?}", rows(&s, p));
            assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
        }

        /// (g) Base [`,`], rewrite [`""`, `,`] (the text inserts r2 before r1); a later edit sets
        /// r2.c1 = L; then the revert of the rewrite conflicts on r2 (`written since`) and
        /// changes nothing.
        #[test]
        fn engine_revert_first_case() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, ",\n");
            let base = s.read(bot).unwrap().version;
            let r = s
                .write_base_grid(bot, base, b"\"\"\n,\n", &GridWriteOptions::default())
                .unwrap();
            let GridWrite::Applied(res) = &r.outcome else {
                panic!("{r:?}")
            };
            let change = res.version;
            let t = table_of(&s, p);
            println!("after the text: rows {:?} {:?}", t.live_rows(), rows(&s, p));
            assert_eq!(t.live_rows(), [k("r2"), k("r1")], "the text's row first");
            op(&s, p, &EditOptions::new(), |t| {
                t.set_cell(&k("r2"), &k("c1"), "L")
            })
            .unwrap();
            let before = rows(&s, p);
            println!("after the edit: {before:?}");
            let out = s.revert(bot, change, false).unwrap();
            println!("revert: {out:?} -> {:?}", rows(&s, p));
            let UndoOutcome::Conflicts { conflicts, .. } = &out else {
                panic!("{out:?}")
            };
            let r2c1 = table::cell_key(&k("r2"), &k("c1"));
            assert!(
                conflicts
                    .iter()
                    .any(|c| c.why.as_deref() == Some("written since")
                        && c.key.as_deref().is_some_and(|x| x == r2c1 || x == "r2")),
                "{conflicts:?}"
            );
            assert_eq!(rows(&s, p), before, "the revert changed nothing");
        }

        /// A live write from an older version can't lose a live edit with no note: it takes the
        /// outside-save path.
        #[test]
        fn a_stale_live_write_overrides_with_a_note() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            let w = f.join(&s, "p2");
            let seen = s.read(w).unwrap().version;
            set(&s, p, "r2", "c1", "LIVE");
            // p2 hadn't seen it, and changes that row
            let r = s
                .write_live_grid(w, seen, b"a,b\nc,X\n", &GridWriteOptions::default())
                .unwrap();
            assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
            assert_eq!(rows(&s, p), ["r1=a,b", "r2=c,X"]);
            let ow = overwritten(&s.take_notes(p).unwrap());
            assert_eq!(ow.len(), 1, "{ow:?}");
            assert_eq!((ow[0].source, ow[0].region.as_str()), ("a write", "r2/c1"));
            let vid = ow[0].version.expect("a version");
            assert_eq!(
                version_holds(&s, p, vid, "r2", "c1").as_deref(),
                Some("LIVE")
            );
            // a row it left exact keeps the live cell, no note
            let seen = s.read(w).unwrap().version;
            set(&s, p, "r1", "c1", "KEPT");
            s.write_live_grid(w, seen, b"a,b\nc,Y\n", &GridWriteOptions::default())
                .unwrap();
            assert_eq!(rows(&s, p), ["r1=KEPT,b", "r2=c,Y"]);
            assert!(overwritten(&s.take_notes(p).unwrap()).is_empty());
            // an expired view: refused
            let r = s
                .write_live_grid(w, 999, b"a,b\n", &GridWriteOptions::default())
                .unwrap();
            assert_eq!(r.outcome, GridWrite::BaseExpired);
        }

        /// `rows drop-deleted` tells whoever wrote a dropped row's cells what it held: it can't
        /// come back.
        #[test]
        fn a_dropped_rows_note_carries_its_content() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            let own = f.join(&s, "p2");
            set(&s, p, "r2", "c1", "MINE");
            op(&s, own, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
            s.drop_deleted(own).unwrap();
            let ow = overwritten(&s.take_notes(p).unwrap());
            assert_eq!(ow.len(), 1, "{ow:?}");
            assert_eq!(ow[0].rule, "dropped_row");
            assert!(ow[0].yours.contains("MINE"), "{}", ow[0].yours);
            assert!(!format!("{:?}", ow[0]).contains("MINE"), "Debug redacts");
            assert!(overwritten(&s.take_notes(own).unwrap()).is_empty());
        }

        /// `rows drop-deleted` over a column someone's cell is hidden in: a `dropped_col` note to
        /// them, with the value, and it comes back from what the note says.
        #[test]
        fn a_dropped_columns_note_carries_its_cells() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let (p, own) = (f.join(&s, "p1"), f.join(&s, "p2"));
            set(&s, p, "r2", "c2", "MINE");
            op(&s, own, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
            s.drop_deleted(own).unwrap();
            let ow = overwritten(&s.take_notes(p).unwrap());
            assert_eq!(ow.len(), 1, "{ow:?}");
            let o = &ow[0];
            assert_eq!(o.rule, "dropped_col");
            assert_eq!(o.region, "col c2");
            assert_eq!(o.col, None, "the column is gone");
            assert_eq!(o.yours, "\"MINE\"");
            assert_eq!(o.source, "rows drop-deleted");
            assert!(
                overwritten(&s.take_notes(own).unwrap()).is_empty(),
                "the dropper's own"
            );
            assert!(!format!("{o:?}").contains("MINE"), "Debug redacts");
            // a drop can't be undone and no version holds a deleted column: the note carries the
            // value itself, and names no version
            assert_eq!(o.version, None);
            assert!(!o.cut);
        }

        /// Two authors in one dropped column: each note holds only the values its author wrote.
        #[test]
        fn a_dropped_columns_notes_hold_only_each_authors_values() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\ne,f\n").unwrap();
            let s = f.open();
            // (the fixture's host knows p1, p2 and bot)
            let (p, q, own) = (f.join(&s, "p1"), f.join(&s, "p2"), f.join(&s, "bot"));
            set(&s, p, "r1", "c2", "PONE");
            set(&s, q, "r2", "c2", "QTWO");
            op(&s, own, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
            s.drop_deleted(own).unwrap();
            let a = overwritten(&s.take_notes(p).unwrap());
            let b = overwritten(&s.take_notes(q).unwrap());
            assert_eq!((a.len(), b.len()), (1, 1), "{a:?} {b:?}");
            assert_eq!(a[0].yours, "\"PONE\"");
            assert_eq!(b[0].yours, "\"QTWO\"");
            assert!(!a[0].to_string().contains("QTWO") && !b[0].to_string().contains("PONE"));
        }

        // ---- §3.4: the text deletes column C, and the live side set a cell in C since V ----

        /// `write --base`: conflict `edited_col` at C naming the cell's writer; nothing applied.
        #[test]
        fn a_text_column_delete_over_a_live_cell_write_base_conflicts() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "a,b,c\nd,e,f\n");
            let base = s.read(bot).unwrap().version;
            set(&s, p, "r1", "c3", "L");
            let r = s
                .write_base_grid(bot, base, b"a,b\nd,e\n", &GridWriteOptions::default())
                .unwrap();
            println!("{r:?}");
            let GridWrite::Conflicts(c) = &r.outcome else {
                panic!("{r:?}")
            };
            assert_eq!(c.len(), 1, "{c:?}");
            assert_eq!(c[0].conflict.rule.as_str(), "edited_col");
            assert_eq!(c[0].conflict.at, table::Region::Col(k("c3")));
            assert_eq!(c[0].written_by.as_deref(), Some("p1"));
            assert_eq!(rows(&s, p), ["r1=a,b,L", "r2=d,e,f"], "nothing applied");
        }

        /// `--partial`: a held column delete is a held layout change, so nothing of the text applies
        /// and the conflicts are plain `write --base`'s (K2 Cores' ruling).
        #[test]
        fn a_text_column_delete_over_a_live_cell_partial_applies_nothing() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "a,b,c\nd,e,f\n");
            let base = s.read(bot).unwrap().version;
            set(&s, p, "r1", "c3", "L");
            let r = s
                .write_base_grid(
                    bot,
                    base,
                    b"a,b\nd,e\n",
                    &GridWriteOptions::default().partial(),
                )
                .unwrap();
            println!("{r:?} -> {:?}", rows(&s, p));
            let GridWrite::Conflicts(c) = &r.outcome else {
                panic!("{r:?}")
            };
            assert!(
                c.iter().any(|c| c.conflict.rule.as_str() == "edited_col"),
                "{c:?}"
            );
            assert!(table_of(&s, p).cols.is_live(&k("c3")), "the delete is held");
            assert_eq!(rows(&s, p), ["r1=a,b,L", "r2=d,e,f"]);
        }

        /// An outside save: the file wins; the live value stays hidden, noted to its author with
        /// the value, and `col restore` brings it back.
        #[test]
        fn a_file_column_delete_over_a_live_cell_is_noted_and_restorable() {
            let f = Fix::new();
            let (s, p, _bot) = with_bot(&f, "a,b,c\nd,e,f\n");
            set(&s, p, "r1", "c3", "L");
            f.save(&s, "a,b\nd,e\n");
            let t = table_of(&s, p);
            assert!(!t.cols.is_live(&k("c3")), "the file wins");
            let n = overwritten(&s.take_notes(p).unwrap());
            assert_eq!(n.len(), 1, "{n:?}");
            assert_eq!((n[0].rule, n[0].region.as_str()), ("edited_col", "col c3"));
            assert_eq!(n[0].yours, "\"L\"");
            op(&s, p, &EditOptions::new(), |t| t.restore_col(&k("c3"))).unwrap();
            assert_eq!(table_of(&s, p).cell(&k("r1"), &k("c3")), Some("L"));
        }

        /// The mixed case under `--partial` (24 in the 300 trial): the text deletes c3 and rewrites
        /// r2 from another row's content; the live side wrote into c3 elsewhere. With the column
        /// delete held, the text's row for r2 must not apply beside r2's old c3 (a row nobody wrote).
        #[test]
        fn partial_with_a_held_column_delete_makes_no_row_nobody_wrote() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(
                &f,
                "v0x0,v0x1,v0x2\nv1x0,v1x1,v1x2\nv2x0,v2x1,v2x2\n,v3x1,\n",
            );
            let base = s.read(bot).unwrap().version;
            set(&s, p, "r3", "c3", "T2");
            set(&s, p, "r4", "c3", "W2");
            let r = s
                .write_base_grid(
                    bot,
                    base,
                    b"v0x0,v0x1\nv2x0,\nv2x0,v2x1\n,v3x1\n",
                    &GridWriteOptions::default().partial(),
                )
                .unwrap();
            let got = rows(&s, p);
            println!("{r:?} -> {got:?}");
            assert!(!got.contains(&"r2=v2x0,,v1x2".to_owned()), "{got:?}");
            // nothing of the text applied, the conflicts plain write --base's
            assert!(matches!(r.outcome, GridWrite::Conflicts(_)), "{r:?}");
            assert_eq!(
                got,
                [
                    "r1=v0x0,v0x1,v0x2",
                    "r2=v1x0,v1x1,v1x2",
                    "r3=v2x0,v2x1,T2",
                    "r4=,v3x1,W2"
                ]
            );
            let plain = {
                let f = Fix::new();
                let (s, p, bot) = with_bot(
                    &f,
                    "v0x0,v0x1,v0x2\nv1x0,v1x1,v1x2\nv2x0,v2x1,v2x2\n,v3x1,\n",
                );
                let base = s.read(bot).unwrap().version;
                set(&s, p, "r3", "c3", "T2");
                set(&s, p, "r4", "c3", "W2");
                s.write_base_grid(
                    bot,
                    base,
                    b"v0x0,v0x1\nv2x0,\nv2x0,v2x1\n,v3x1\n",
                    &GridWriteOptions::default(),
                )
                .unwrap()
            };
            let rules = |o: &GridWrite| match o {
                GridWrite::Conflicts(c) => {
                    c.iter().map(|c| c.conflict.to_string()).collect::<Vec<_>>()
                }
                _ => Vec::new(),
            };
            assert_eq!(
                rules(&r.outcome),
                rules(&plain.outcome),
                "plain write --base's conflicts"
            );
        }

        /// The same rule for a held column move: the text moves c1 to the end, the live side
        /// deleted c1 (so the move is held: `deleted_col`); `--partial` applies nothing.
        #[test]
        fn partial_with_a_held_column_move_applies_nothing() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "a,b,c\nd,e,f\n");
            let base = s.read(bot).unwrap().version;
            op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c1"))).unwrap();
            let before = rows(&s, p);
            let r = s
                .write_base_grid(
                    bot,
                    base,
                    b"b,c,a\nE,f,d\n",
                    &GridWriteOptions::default().partial(),
                )
                .unwrap();
            println!("move: {r:?} -> {:?}", rows(&s, p));
            assert!(matches!(r.outcome, GridWrite::Conflicts(_)), "{r:?}");
            assert_eq!(rows(&s, p), before, "nothing applied");
        }

        /// FOUND (the 300 trial at eca1a63, Rewrite/Outside): the live side writes into r1/c3 and
        /// deletes c3; the file changes r1 elsewhere and c3 on another row, so c3 comes back. r1
        /// was changed by both, so the file's row should win whole: `a,X,c`, the live `T` noted.
        /// If the column comes back with its hidden cells, r1 is `a,X,T`: a row nobody wrote.
        #[test]
        fn a_file_bringing_back_a_live_deleted_column_wins_its_rows_whole() {
            under_merge_mutant(|| {
                let f = Fix::new();
                std::fs::write(f.file(), "a,b,c\nd,e,f\n").unwrap();
                let s = f.open();
                let p = f.join(&s, "p1");
                set(&s, p, "r1", "c3", "T");
                op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c3"))).unwrap();
                f.save(&s, "a,X,c\nd,e,F\n");
                let got = rows(&s, p);
                let n = overwritten(&s.take_notes(p).unwrap());
                println!("rows {got:?} notes {n:?} audit {:?}", f.overrides());
                assert!(
                    !got.contains(&"r1=a,X,T".to_owned()),
                    "a row nobody wrote: {got:?}"
                );
                assert_eq!(got[0], "r1=a,X,c", "the file's row, whole");
            })
        }

        /// Mirror 1: the file deletes a column the live side wrote into, on a row both changed: the
        /// column goes, the value stays hidden, and its author gets an `edited_col` note with it.
        #[test]
        fn mirror_a_file_column_delete_over_a_whole_rows_live_cell() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b,c\nd,e,f\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c3", "T");
            set(&s, p, "r1", "c1", "U");
            f.save(&s, "a,X\nd,e\n");
            let t = table_of(&s, p);
            let n = overwritten(&s.take_notes(p).unwrap());
            println!("mirror1 rows {:?} notes {n:?}", rows(&s, p));
            assert!(!t.cols.is_live(&k("c3")), "the column goes");
            assert_eq!(
                t.cell(&k("r1"), &k("c3")),
                Some("T"),
                "the value stays hidden"
            );
            assert!(
                cells_of(&n).iter().any(|c| c.0 == "col c3"
                    && c.1 == "edited_col"
                    && c.2.split(", ").any(|x| x == "\"T\"")),
                "{n:?}"
            );
            assert_eq!(rows(&s, p)[0], "r1=a,X", "the file's row, whole");
        }

        /// Mirror 2: the file keeps a column the live side moved, on a row both changed: the
        /// file's row wins whole, by key, no field shifted.
        #[test]
        fn mirror_a_file_over_a_live_column_move_on_a_whole_row() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b,c\nd,e,f\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c2", "T");
            op(&s, p, &EditOptions::new(), |t| {
                t.move_col(&k("c3"), Anchor::Start)
            })
            .unwrap();
            f.save(&s, "a,b,X\nd,e,f\n");
            let t = table_of(&s, p);
            println!("mirror2 rows {:?} cols {:?}", rows(&s, p), t.live_cols());
            assert_eq!(t.cell(&k("r1"), &k("c1")), Some("a"));
            assert_eq!(
                t.cell(&k("r1"), &k("c2")),
                Some("b"),
                "the file's row wins whole"
            );
            assert_eq!(t.cell(&k("r1"), &k("c3")), Some("X"), "by key, not shifted");
            assert_eq!(t.cell(&k("r2"), &k("c3")), Some("f"));
            let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            assert!(n.iter().any(|c| c.0 == "r1/c2" && c.2 == "\"T\""), "{n:?}");
        }

        /// The outside-merge property's shrunk case (base-a2 54b45362 at f5931e3): r1/c1 is `""`,
        /// the live side unsets it (absent: the same bytes) and sets r1/c2; the file sets r1/c1.
        /// Both changed r1, so the file's row wins whole; the unset is no live edit of c1 (the base
        /// row with only that cell switched renders the same bytes), so the only noted cell and
        /// the only audit event are r1/c2's.
        #[test]
        fn a_byte_neutral_live_unset_in_a_row_both_changed_is_not_noted() {
            let f = Fix::new();
            std::fs::write(
                f.file(),
                ",v0x1,v0x2\nv1x0,v1x1,v1x2\nv2x0,v2x1,v2x2\nv3x0,v3x1,v3x2\nv4x0,v4x1,v4x2\n",
            )
            .unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            s.edit_ops(
                p,
                vec![Edit::Map {
                    root: CELLS.into(),
                    ops: vec![MapOp::Delete {
                        key: table::cell_key(&k("r1"), &k("c1")),
                    }],
                }],
                &EditOptions::new(),
            )
            .unwrap();
            set(&s, p, "r1", "c2", "L0v0");
            f.save(
                &s,
                "F0v0,v0x1,v0x2\nv1x0,v1x1,v1x2\nv2x0,v2x1,v2x2\nv3x0,v3x1,v3x2\nv4x0,v4x1,v4x2\n",
            );
            let got = rows(&s, p);
            let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            let audit = f.overrides();
            println!("rows {got:?} notes {n:?} audit {audit:?}");
            assert_eq!(got[0], "r1=F0v0,v0x1,v0x2", "the file's row, whole");
            let noted: Vec<_> = n
                .iter()
                .map(|c| (c.0.as_str(), c.1, c.2.as_str()))
                .collect();
            assert_eq!(noted, [("r1/c2", "cell", "\"L0v0\"")], "one noted cell");
            assert_eq!(audit.len(), 1, "{audit:?}");
            assert_eq!(
                (audit[0].0.as_str(), audit[0].1.as_str()),
                ("cell", "r1/c2")
            );
        }

        /// `merge3_with` is shared with `write --base`: the text keeps C unchanged on a row it
        /// changed elsewhere; the live side wrote C on that row and then deleted C. Both changed
        /// the row: `edited_row`, nothing of it applies; `--partial` skips it and applies the rest
        /// (the text didn't touch C: no column conflict). Fails before `by_cell` reads the
        /// text's columns (it read the live side's, so the write into C was missed).
        #[test]
        fn write_base_a_live_write_then_delete_of_a_column_the_text_kept_is_edited_row() {
            under_merge_mutant(|| {
                for partial in [false, true] {
                    let f = Fix::new();
                    let (s, p, bot) = with_bot(&f, "a,b,c\nd,e,f\n");
                    let base = s.read(bot).unwrap().version;
                    set(&s, p, "r1", "c3", "T");
                    op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c3"))).unwrap();
                    let before = rows(&s, p);
                    let opts = if partial {
                        GridWriteOptions::default().partial()
                    } else {
                        GridWriteOptions::default()
                    };
                    let r = s
                        .write_base_grid(bot, base, b"a,X,c\nD,e,f\n", &opts)
                        .unwrap();
                    let got = rows(&s, p);
                    println!("partial={partial}: {r:?} -> {got:?}");
                    let held = if partial {
                        assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
                        r.skipped.clone()
                    } else {
                        let GridWrite::Conflicts(c) = &r.outcome else {
                            panic!("partial={partial}: {r:?}")
                        };
                        assert_eq!(got, before, "nothing applied");
                        c.clone()
                    };
                    let held: Vec<_> = held
                        .iter()
                        .map(|c| (c.conflict.rule.as_str(), c.conflict.at.clone()))
                        .collect();
                    assert_eq!(
                        held,
                        [("edited_row", table::Region::Row(k("r1")))],
                        "partial={partial}"
                    );
                    if partial {
                        assert_eq!(got, ["r1=a,b", "r2=D,e"], "the rest applied, r1 skipped");
                    }
                    let t = table_of(&s, p);
                    assert!(!t.cols.is_live(&k("c3")), "C stays deleted");
                    assert_eq!(t.cell(&k("r1"), &k("c3")), Some("T"), "still hidden");
                }
            })
        }

        /// K2 Engine's three `--partial` probes with a held column change (job c3d226ea at
        /// 6886198): `--partial` applies nothing, the doc is unchanged, and its conflicts equal
        /// plain `write --base`'s.
        fn engine_partial_applies_nothing(
            name: &str,
            file: &str,
            live: impl Fn(&Session, ConnId),
            text: &[u8],
        ) {
            let run = |partial: bool| {
                let f = Fix::new();
                let (s, p, bot) = with_bot(&f, file);
                let base = s.read(bot).unwrap().version;
                live(&s, p);
                let before = (
                    rows(&s, p),
                    table_of(&s, p).live_cols(),
                    s.read(bot).unwrap().version,
                );
                let opts = if partial {
                    GridWriteOptions::default().partial()
                } else {
                    GridWriteOptions::default()
                };
                let r = s.write_base_grid(bot, base, text, &opts).unwrap();
                let after = (
                    rows(&s, p),
                    table_of(&s, p).live_cols(),
                    s.read(bot).unwrap().version,
                );
                println!("{name} partial={partial}: {r:?} -> {after:?}");
                (r, before, after)
            };
            let conflicts = |o: &GridWrite| match o {
                GridWrite::Conflicts(c) => {
                    let mut v: Vec<_> = c.iter().map(|c| c.conflict.to_string()).collect();
                    v.sort();
                    v
                }
                _ => Vec::new(),
            };
            let (plain, ..) = run(false);
            let (r, before, after) = run(true);
            assert!(
                matches!(r.outcome, GridWrite::Conflicts(_)) && r.skipped.is_empty(),
                "{name}: --partial must apply nothing: {r:?}"
            );
            assert_eq!(after, before, "{name}: the doc is unchanged");
            assert!(
                !conflicts(&plain.outcome).is_empty(),
                "{name}: plain write --base conflicts"
            );
            assert_eq!(
                conflicts(&r.outcome),
                conflicts(&plain.outcome),
                "{name}: plain write --base's conflicts"
            );
        }

        /// K2 Engine 1 (delete): at 6886198 `--partial` gave [`,z`, `a,a`]; plain: `edited_col` c2.
        #[test]
        fn engine_partial_held_column_delete() {
            engine_partial_applies_nothing(
                "delete",
                ",\n,a\n",
                |s, p| set(s, p, "r1", "c2", "z"),
                b"\"\"\na\n",
            );
        }

        /// K2 Engine 2 (insert): at 6886198 `--partial` brought the live-deleted column back.
        #[test]
        fn engine_partial_insert_over_a_live_column_delete() {
            engine_partial_applies_nothing(
                "insert",
                ",\n,a\n",
                |s, p| {
                    op(s, p, &EditOptions::new(), |t| t.delete_col(&k("c1"))).unwrap();
                },
                b"x,,\nx,,a\n",
            );
        }

        /// K2 Engine 3 (move): at 6886198 `--partial` applied the text's other edit.
        #[test]
        fn engine_partial_move_over_a_live_column_delete() {
            engine_partial_applies_nothing(
                "move",
                ",\n,a\n",
                |s, p| {
                    op(s, p, &EditOptions::new(), |t| t.delete_col(&k("c1"))).unwrap();
                },
                b",\ny,\n",
            );
        }

        /// Ruling (a): the text writes into C, which the live side deleted: `--partial` applies
        /// nothing, with plain `write --base`'s conflicts.
        #[test]
        fn partial_writing_into_a_live_deleted_column_applies_nothing() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "a,b,c\nd,e,f\n");
            let base = s.read(bot).unwrap().version;
            op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
            let before = rows(&s, p);
            let body = b"a,B,c\nD,e,f\n";
            let r = s
                .write_base_grid(bot, base, body, &GridWriteOptions::default().partial())
                .unwrap();
            println!("{r:?}");
            let GridWrite::Conflicts(c) = &r.outcome else {
                panic!("{r:?}")
            };
            assert!(
                c.iter().any(|c| c.conflict.rule.as_str() == "deleted_col"),
                "{c:?}"
            );
            assert_eq!(rows(&s, p), before, "nothing applied");
        }

        /// Ruling (a): the text renames C, which the live side deleted: `--partial` applies nothing.
        #[test]
        fn partial_renaming_a_live_deleted_column_applies_nothing() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "n,v,w\na,b,c\nd,e,f\n");
            op(&s, p, &EditOptions::new(), |t| t.set_header(true)).unwrap();
            let base = s.read(bot).unwrap().version;
            op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
            let before = rows(&s, p);
            let r = s
                .write_base_grid(
                    bot,
                    base,
                    b"n,V2,w\nA,b,c\nd,e,f\n",
                    &GridWriteOptions::default().partial(),
                )
                .unwrap();
            println!("{r:?}");
            let GridWrite::Conflicts(c) = &r.outcome else {
                panic!("{r:?}")
            };
            assert!(
                c.iter().any(|c| c.conflict.rule.as_str() == "deleted_col"),
                "{c:?}"
            );
            assert_eq!(rows(&s, p), before, "nothing applied");
        }

        /// Ruling (a): the live side deletes a middle column C, the text edits other columns only:
        /// `--partial` and plain `write --base` both apply, every field in its own column by key.
        #[test]
        fn a_live_column_delete_the_text_leaves_alone_applies_by_key() {
            for partial in [false, true] {
                let f = Fix::new();
                let (s, p, bot) = with_bot(&f, "a,b,c\nd,e,f\n");
                let base = s.read(bot).unwrap().version;
                op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
                let opts = if partial {
                    GridWriteOptions::default().partial()
                } else {
                    GridWriteOptions::default()
                };
                let r = s
                    .write_base_grid(bot, base, b"A,b,c\nd,e,F\n", &opts)
                    .unwrap();
                assert!(
                    matches!(r.outcome, GridWrite::Applied(_)) && r.skipped.is_empty(),
                    "{r:?}"
                );
                let t = table_of(&s, p);
                assert_eq!(t.cell(&k("r1"), &k("c1")), Some("A"));
                assert_eq!(t.cell(&k("r2"), &k("c3")), Some("F"));
                assert_eq!(t.cell(&k("r1"), &k("c3")), Some("c"), "not shifted");
                assert!(!t.cols.is_live(&k("c2")));
                assert_eq!(rows(&s, p), ["r1=A,c", "r2=d,F"]);
            }
        }

        /// A column insert by the text next to a column the live side deleted: if a column
        /// conflict holds it, nothing of the text applies; if not, the text applies and the
        /// live-deleted column stays deleted (no column comes back by key).
        #[test]
        fn partial_with_a_column_insert_over_a_live_column_delete() {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, "a,b\nd,e\n");
            let base = s.read(bot).unwrap().version;
            op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c1"))).unwrap();
            let before = rows(&s, p);
            let r = s
                .write_base_grid(
                    bot,
                    base,
                    b"a,N,b\nd,M,e\n",
                    &GridWriteOptions::default().partial(),
                )
                .unwrap();
            let after = rows(&s, p);
            println!("insert: {r:?} -> {after:?}");
            let col_held = match &r.outcome {
                GridWrite::Conflicts(c) => c
                    .iter()
                    .any(|c| matches!(c.conflict.at, table::Region::Col(_))),
                _ => r
                    .skipped
                    .iter()
                    .any(|c| matches!(c.conflict.at, table::Region::Col(_))),
            };
            if col_held {
                assert!(matches!(r.outcome, GridWrite::Conflicts(_)), "{r:?}");
                assert_eq!(after, before, "nothing applied");
            } else {
                // nothing held: the text applied, and no live-deleted column came back by key
                assert!(
                    matches!(r.outcome, GridWrite::Applied(_)) && r.skipped.is_empty(),
                    "{r:?}"
                );
                let t = table_of(&s, p);
                assert!(
                    !t.cols.is_live(&k("c1")),
                    "c1 stays deleted: {:?}",
                    t.live_cols()
                );
                let new: Vec<Key> = t
                    .live_cols()
                    .into_iter()
                    .filter(|c| *c != k("c1") && *c != k("c2"))
                    .collect();
                assert_eq!(
                    new.len(),
                    1,
                    "the text's one new column: {:?}",
                    t.live_cols()
                );
                assert_eq!(t.cell(&k("r1"), &new[0]), Some("N"));
                assert_eq!(t.cell(&k("r2"), &new[0]), Some("M"));
                assert_eq!(t.cell(&k("r1"), &k("c2")), Some("b"));
                assert_eq!(t.cell(&k("r2"), &k("c2")), Some("e"));
            }
        }

        /// K2 Engine (fix 1): base [`,,`]; p2 sets r1.c1 := z; the owner deletes c1 and drops: a
        /// plain column cell in a row that stays: a `dropped_col` note to p2.
        #[test]
        fn engine_a_dropped_columns_plain_cell_is_noted() {
            let f = Fix::new();
            std::fs::write(f.file(), ",,\n").unwrap();
            let s = f.open();
            let (own, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            set(&s, p2, "r1", "c1", "z");
            op(&s, own, &EditOptions::new(), |t| t.delete_col(&k("c1"))).unwrap();
            s.drop_deleted(own).unwrap();
            let n = overwritten(&s.take_notes(p2).unwrap());
            assert_eq!(n.len(), 1, "{n:?}");
            assert_eq!((n[0].rule, n[0].region.as_str()), ("dropped_col", "col c1"));
            assert_eq!(n[0].yours, "\"z\"");
        }

        /// K2 Engine (fix 3): base [blank, then `,` for the columns], file `,a`; live c2 := `""` on
        /// the blank row: its bytes changed (blank to `,`), so a live edit: the file wins whole,
        /// with a note.
        #[test]
        fn engine_a_live_empty_on_a_blank_row_is_an_edit() {
            let f = Fix::new();
            std::fs::write(f.file(), "\n,\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c2", "");
            assert_eq!(
                String::from_utf8(s.read(p).unwrap().bytes).unwrap(),
                ",\n,\n"
            );
            f.save(&s, ",a\n,\n");
            assert_eq!(
                String::from_utf8(s.read(p).unwrap().bytes).unwrap(),
                ",a\n,\n"
            );
            let n = overwritten(&s.take_notes(p).unwrap());
            assert!(!n.is_empty(), "a note: {:?}", f.overrides());
            // an empty/absent switch that keeps the row's bytes is still no edit
            let f = Fix::new();
            std::fs::write(f.file(), "x,\n,\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c2", "");
            assert_eq!(
                String::from_utf8(s.read(p).unwrap().bytes).unwrap(),
                "x,\n,\n"
            );
            f.save(&s, "x,a\n,\n");
            assert!(overwritten(&s.take_notes(p).unwrap()).is_empty());
        }

        /// K2 Engine (`restore_over_a_hidden_value`): p2 sets r1/c1 := z, then deletes r1 (or c1,
        /// or nothing); p1 restores a version from before p2's set. The restore takes z (r1 comes
        /// back with c1 = `a`, or z goes): p2 gets a note carrying `"z"` in every case.
        fn restore_over_a_hidden_value(delete: &str) {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let (own, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            let v = s.name_version(own, "v0", None).unwrap();
            set(&s, p2, "r1", "c1", "z");
            match delete {
                "row" => {
                    op(&s, p2, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
                }
                "col" => {
                    op(&s, p2, &EditOptions::new(), |t| t.delete_col(&k("c1"))).unwrap();
                }
                _ => {}
            }
            s.restore(own, v).unwrap();
            let t = table_of(&s, own);
            let n = cells_of(&overwritten(&s.take_notes(p2).unwrap()));
            println!(
                "{delete}: rows {:?} r1/c1 {:?}; notes to p2 {n:?}",
                rows(&s, own),
                t.cell(&k("r1"), &k("c1"))
            );
            assert_eq!(
                t.cell(&k("r1"), &k("c1")),
                Some("a"),
                "{delete}: the version's value"
            );
            assert!(
                n.iter().any(|c| c.0 == "r1/c1" && c.2 == "\"z\""),
                "{delete}: p2's z, noted: {n:?}"
            );
        }

        #[test]
        fn restore_over_a_hidden_value_in_a_deleted_row_is_noted() {
            restore_over_a_hidden_value("row");
        }

        #[test]
        fn restore_over_a_hidden_value_in_a_deleted_column_is_noted() {
            restore_over_a_hidden_value("col");
        }

        #[test]
        fn restore_over_a_live_value_is_noted() {
            restore_over_a_hidden_value("none");
        }

        /// K2 Engine (18 cases): base `q,`; the live side undoes q (c1 absent) and sets c2 := z,
        /// so it renders `,z`; the text's row is `,z` too. Rows that render the same bytes are the
        /// same row: `write --base` applies with no conflict, the cells the text's. An outside
        /// save of the same bytes overwrites nothing (no note).
        fn same_rendering(outside: bool) {
            let f = Fix::new();
            let (s, p, bot) = with_bot(&f, ",\n");
            // c1 absent (the same bytes), then q set over it: base `q,`
            s.edit_ops(
                p,
                vec![Edit::Map {
                    root: CELLS.into(),
                    ops: vec![MapOp::Delete {
                        key: table::cell_key(&k("r1"), &k("c1")),
                    }],
                }],
                &EditOptions::new(),
            )
            .unwrap();
            set(&s, p, "r1", "c1", "q");
            let base = s.read(bot).unwrap().version;
            if outside {
                // the file holds `q,` (the save's base), as `write --base`'s V does
                f.advance(2_000);
                assert!(s.flush(p).unwrap().written);
                assert_eq!(f.bytes(), "q,\n");
            }
            let u = s.undo(p, 1).unwrap();
            set(&s, p, "r1", "c2", "z");
            let t = table_of(&s, p);
            println!(
                "live: undo {u:?}; r1 {:?} {:?} rows {:?}",
                t.cell(&k("r1"), &k("c1")),
                t.cell(&k("r1"), &k("c2")),
                rows(&s, p)
            );
            assert_eq!(
                t.cell(&k("r1"), &k("c1")),
                None,
                "the undo leaves c1 absent"
            );
            assert_eq!(rows(&s, p), ["r1=,z"], "the live row renders `,z`");
            if outside {
                f.save(&s, ",z\n");
                let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
                println!("outside save: rows {:?} notes {n:?}", rows(&s, p));
                assert!(n.is_empty(), "nothing overwritten: {n:?}");
            } else {
                let r = s
                    .write_base_grid(bot, base, b",z\n", &GridWriteOptions::default())
                    .unwrap();
                println!("write --base: {r:?} -> {:?}", rows(&s, p));
                assert!(
                    matches!(r.outcome, GridWrite::Applied(_)) && r.skipped.is_empty(),
                    "{r:?}"
                );
            }
            let t = table_of(&s, p);
            assert_eq!(rows(&s, p), ["r1=,z"]);
            assert_eq!(t.cell(&k("r1"), &k("c2")), Some("z"));
            assert_eq!(t.cell(&k("r1"), &k("c1")).unwrap_or(""), "");
        }

        #[test]
        fn write_base_a_text_row_rendering_as_the_live_row_is_no_conflict() {
            same_rendering(false);
        }

        #[test]
        fn an_outside_save_rendering_as_the_live_row_overwrites_nothing() {
            same_rendering(true);
        }

        /// K2 Engine (376 cases): base [`a,b,c`, `d,e,f`]; the live side sets r1/c3 := T and deletes
        /// c3; the file saves [`A,c`, `d,f`] (column 2 dropped: a non-exact mapping). Both changed
        /// r1, so the file's row wins whole: T is overridden and noted to p1, and restoring c3
        /// shows the file's row, never `A,T` (a row nobody wrote).
        #[test]
        fn a_file_over_a_live_write_then_column_delete_under_a_non_exact_mapping_wins_whole() {
            under_merge_mutant(|| {
                let f = Fix::new();
                std::fs::write(f.file(), "a,b,c\nd,e,f\n").unwrap();
                let s = f.open();
                let p = f.join(&s, "p1");
                set(&s, p, "r1", "c3", "T");
                op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c3"))).unwrap();
                f.save(&s, "A,c\nd,f\n");
                let t = table_of(&s, p);
                let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
                println!(
                    "after the save: rows {:?} cols {:?} deleted {:?} cells {:?}; notes {n:?}",
                    rows(&s, p),
                    t.live_cols(),
                    t.cols.deleted().collect::<Vec<_>>(),
                    t.cells
                );
                assert!(
                    n.iter().any(|c| c.2.split(", ").any(|x| x == "\"T\"")),
                    "T overridden with a note to p1: {n:?}"
                );
                // the file dropped c2, c3 stays deleted (the file didn't change it), and its
                // hidden cell in r1 holds the file's `c`, not T
                assert!(!t.cols.is_live(&k("c2")), "c2 deleted by the file");
                assert!(!t.cols.is_live(&k("c3")), "c3 still deleted");
                assert_eq!(rows(&s, p), ["r1=A", "r2=d"]);
                assert_eq!(
                    t.cell(&k("r1"), &k("c3")),
                    Some("c"),
                    "the file's value, hidden"
                );
                op(&s, p, &EditOptions::new(), |t| t.restore_col(&k("c3"))).unwrap();
                let got = rows(&s, p);
                println!("after restoring c3: {got:?}");
                assert!(
                    !got.iter().any(|r| r.starts_with("r1=") && r.contains('T')),
                    "a row nobody wrote: {got:?}"
                );
            })
        }

        /// §2.9.1's "a column the live side deleted, the file edited: it comes back". A file can only
        /// write into a column it identifies, so the header names them: `n1,n2,n3` / `a,b,c` /
        /// `d,e,f`; the live side sets r2/c3 := T and deletes c3; the file saves `n1,n3` / `A,c` /
        /// `d,F` (c2 dropped, F written into c3 on r3). c3 comes back with F; r2's row wins whole
        /// (its c3 the file's `c`), and T is noted to p1.
        #[test]
        fn a_file_writing_into_a_live_deleted_column_it_names_brings_it_back() {
            let f = Fix::new();
            std::fs::write(f.file(), "n1,n2,n3\na,b,c\nd,e,f\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            op(&s, p, &EditOptions::new(), |t| t.set_header(true)).unwrap();
            f.advance(2_000);
            assert!(s.flush(p).unwrap().written || f.bytes() == "n1,n2,n3\na,b,c\nd,e,f\n");
            set(&s, p, "r2", "c3", "T");
            op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c3"))).unwrap();
            f.save(&s, "n1,n3\nA,c\nd,F\n");
            let t = table_of(&s, p);
            let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            println!(
                "rows {:?} cols {:?} cells {:?}; notes {n:?}",
                rows(&s, p),
                t.live_cols(),
                t.cells
            );
            assert!(t.cols.is_live(&k("c3")), "c3 comes back");
            assert!(!t.cols.is_live(&k("c2")), "c2 dropped by the file");
            assert_eq!(t.cell(&k("r2"), &k("c3")), Some("c"), "r2's row, whole");
            assert_eq!(t.cell(&k("r3"), &k("c3")), Some("F"));
            assert!(
                n.iter().any(|c| c.2.split(", ").any(|x| x == "\"T\"")),
                "T noted: {n:?}"
            );
        }

        /// The header-less form of the test above (K2 Cores' ruling: after a width change, content
        /// pairing needs an exact match (of a column's values, sorted), and an inexact one falls back
        /// to position, since a guess doesn't decide; `table::reconcile`). Base `a,b,c` / `d,e,f`; the live side sets r1/c3 :=
        /// T and deletes c3; the file saves `A,c` / `d,F`. Its second field `[c, F]` matches no base
        /// column exactly, so c1 and c2 pair and c3 is the column it dropped: c3 is deleted on both
        /// sides, T stays hidden by its own author's delete, the file never touches c3, and no live
        /// value is overwritten (no note). r1 isn't both-changed on the columns the file keeps, so
        /// A and c apply cell by cell. With a header the file can name c3: the test above.
        #[test]
        fn a_headerless_file_edit_of_a_live_deleted_column_falls_back_to_position() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b,c\nd,e,f\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c3", "T");
            op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c3"))).unwrap();
            f.save(&s, "A,c\nd,F\n");
            let t = table_of(&s, p);
            let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            println!(
                "rows {:?} cols {:?} cells {:?}; notes {n:?}",
                rows(&s, p),
                t.live_cols(),
                t.cells
            );
            assert_eq!(t.live_cols(), [k("c1"), k("c2")], "c1 and c2 paired");
            assert!(!t.cols.is_live(&k("c3")), "c3 deleted on both sides");
            assert_eq!(rows(&s, p), ["r1=A,c", "r2=d,F"]);
            assert_eq!(
                t.cell(&k("r1"), &k("c3")),
                Some("T"),
                "T hidden by its own delete"
            );
            assert!(n.is_empty(), "no live value overwritten: {n:?}");
        }

        /// After the header-less case above, p1 restores c3, the column both sides deleted: it brings
        /// back what its cells last held (r1's T, p1's own; r2's `f`), with no note. Benign (K2
        /// Cores' ruling): the person restoring is the one who deleted it.
        #[test]
        fn a_column_deleted_by_both_sides_restores_what_its_cells_last_held() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b,c\nd,e,f\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c3", "T");
            op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c3"))).unwrap();
            f.save(&s, "A,c\nd,F\n");
            let _ = s.take_notes(p).unwrap();
            op(&s, p, &EditOptions::new(), |t| t.restore_col(&k("c3"))).unwrap();
            let got = rows(&s, p);
            let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            println!("after the restore: {got:?}; notes {n:?}");
            assert_eq!(got, ["r1=A,c,T", "r2=d,F,f"]);
            assert!(n.is_empty(), "{n:?}");
        }

        /// Option 1 (K2 Cores' ruling): with no header and the width unchanged, columns pair by
        /// position, so a header-less pure column move reads as value rewrites. The live side
        /// edited r1 since: `write --base` gives `edited_row` at r1; an outside save gives the
        /// file's rows whole, the live value noted. No column moves; nothing is lost.
        #[test]
        fn a_headerless_column_swap_at_the_same_width_reads_as_value_rewrites() {
            for outside in [false, true] {
                let f = Fix::new();
                let (s, p, bot) = with_bot(&f, "a,b\nc,d\n");
                let base = s.read(bot).unwrap().version;
                set(&s, p, "r1", "c1", "A");
                let body = "b,a\nd,c\n";
                if outside {
                    f.save(&s, body);
                    let t = table_of(&s, p);
                    let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
                    println!(
                        "outside: rows {:?} cols {:?}; notes {n:?}",
                        rows(&s, p),
                        t.live_cols()
                    );
                    assert_eq!(t.live_cols(), [k("c1"), k("c2")], "no column moved");
                    assert_eq!(rows(&s, p), ["r1=b,a", "r2=d,c"]);
                    assert!(
                        n.iter().any(|c| c.0 == "r1/c1" && c.2 == "\"A\""),
                        "the live A noted: {n:?}"
                    );
                } else {
                    let r = s
                        .write_base_grid(bot, base, body.as_bytes(), &GridWriteOptions::default())
                        .unwrap();
                    println!("write --base: {r:?} -> {:?}", rows(&s, p));
                    let GridWrite::Conflicts(c) = &r.outcome else {
                        panic!("{r:?}")
                    };
                    let at: Vec<_> = c
                        .iter()
                        .map(|c| (c.conflict.rule.as_str(), c.conflict.at.clone()))
                        .collect();
                    assert_eq!(at, [("edited_row", table::Region::Row(k("r1")))]);
                    assert_eq!(rows(&s, p), ["r1=A,b", "r2=c,d"], "nothing applied");
                }
            }
        }

        /// The width rule on a ragged header-less doc (K2 Cores): the text's width is its widest
        /// row, the base's its live columns. The longest row growing by a field is a width change
        /// (a new column); a shorter row growing within that width is not (by position: a cell).
        #[test]
        fn a_ragged_text_width_is_its_widest_row() {
            for outside in [false, true] {
                for (tag, body) in [
                    ("longest grows", "a,b,c,Z\nd,e\nx\n"),
                    ("shorter grows", "a,b,c\nd,e,E\nx\n"),
                ] {
                    let f = Fix::new();
                    let (s, p, bot) = with_bot(&f, "a,b,c\nd,e\nx\n");
                    let base = s.read(bot).unwrap().version;
                    assert_eq!(table_of(&s, p).shape, Shape::Ragged);
                    if outside {
                        f.save(&s, body);
                    } else {
                        let r = s
                            .write_base_grid(
                                bot,
                                base,
                                body.as_bytes(),
                                &GridWriteOptions::default(),
                            )
                            .unwrap();
                        assert!(matches!(r.outcome, GridWrite::Applied(_)), "{tag}: {r:?}");
                    }
                    let t = table_of(&s, p);
                    println!(
                        "outside={outside} {tag}: cols {:?} cells {:?}",
                        t.live_cols(),
                        t.cells
                    );
                    if tag == "longest grows" {
                        assert_eq!(t.live_cols().len(), 4, "{tag}: a new column");
                        assert_eq!(&t.live_cols()[..3], [k("c1"), k("c2"), k("c3")], "{tag}");
                        assert_eq!(t.cell(&k("r1"), &t.live_cols()[3]), Some("Z"), "{tag}");
                        assert_eq!(t.cell(&k("r1"), &k("c3")), Some("c"), "{tag}");
                    } else {
                        assert_eq!(
                            t.live_cols(),
                            [k("c1"), k("c2"), k("c3")],
                            "{tag}: no new column"
                        );
                        assert_eq!(t.cell(&k("r2"), &k("c3")), Some("E"), "{tag}");
                        assert_eq!(t.cell(&k("r1"), &k("c3")), Some("c"), "{tag}");
                    }
                }
            }
        }

        /// K2 Engine's ragged width cases (`ragged_width_cases`, crates/base/tests/
        /// reconcile_pairing_diff.rs on its branch engine/reconcile-diff-617343c), the live edit
        /// "r1 last := T, then delete that column", on the outside save: the file keeps that column
        /// and changes r1, so r1's row wins whole. p1 gets a note carrying `"T"`; the rows render as
        /// the file's (without the column the live side deleted); and after `col restore`, the
        /// column shows the file's value, no T anywhere.
        fn ragged_live_write_then_column_delete(
            base: &str,
            file: &str,
            before: &str,
            restored: &str,
        ) {
            let f = Fix::new();
            std::fs::write(f.file(), base).unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            let last = table_of(&s, p).live_cols().last().unwrap().clone();
            set(&s, p, "r1", last.as_str(), "T");
            op(&s, p, &EditOptions::new(), |t| t.delete_col(&last)).unwrap();
            f.save(&s, file);
            let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            let text = |s: &Session| String::from_utf8(s.read(p).unwrap().bytes).unwrap();
            println!("{file:?}: text {:?} notes {n:?}", text(&s));
            assert!(
                n.iter().any(|c| c.2.split(", ").any(|x| x == "\"T\"")),
                "{file:?}: a note carrying T: {n:?}"
            );
            assert_eq!(text(&s), before, "{file:?}: the file's rows");
            op(&s, p, &EditOptions::new(), |t| t.restore_col(&last)).unwrap();
            let t = table_of(&s, p);
            println!(
                "{file:?}: after the restore {:?} cells {:?}",
                text(&s),
                t.cells
            );
            assert_eq!(
                text(&s),
                restored,
                "{file:?}: the file's value in the restored column"
            );
            assert!(
                !t.cells.values().any(|v| v == "T"),
                "{file:?}: no T: {:?}",
                t.cells
            );
        }

        /// (a) Only the longest row grows (a width change: content pairing; a new column).
        #[test]
        fn ragged_a_only_the_longest_row_grows_wins_whole_over_a_hidden_live_value() {
            // the file's new column x is placed after its live left neighbour (c2), before the
            // column deleted there (`place_after`, table/merge.rs:1019-1046); the restore revives
            // c3 in its kept place, so it returns after x: r1/c3 the file's `c`, r2/c3 absent
            ragged_live_write_then_column_delete(
                "a,b,c\nd,e\n",
                "a,b,c,x\nd,e\n",
                "a,b,x\nd,e\n",
                "a,b,x,c\nd,e\n",
            );
        }

        /// (b) The text's widest row equals the base's column count, other rows short (by position).
        #[test]
        fn ragged_b_same_width_short_rows_wins_whole_over_a_hidden_live_value() {
            ragged_live_write_then_column_delete(
                "a,b,c\nd,e\n",
                "a,B,c\nd\n",
                "a,B\nd\n",
                "a,B,c\nd\n",
            );
        }

        /// (e) The 376 shape (base three columns, text two fields: content pairing keeps c1 and c3).
        #[test]
        fn ragged_e_the_376_shape_wins_whole_over_a_hidden_live_value() {
            ragged_live_write_then_column_delete(
                "a,b,c\nd,e,f\n",
                "A,c\nd,f\n",
                "A\nd\n",
                "A,c\nd,f\n",
            );
        }

        /// After the whole-row override of a hidden cell (the case above with `A,c`), restoring the
        /// column isn't refused, and a later drop works (no stray cell under a dropped key).
        #[test]
        fn a_restore_and_a_drop_after_a_whole_row_override_of_a_hidden_cell() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b,c\nd,e,f\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c3", "T");
            op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c3"))).unwrap();
            f.save(&s, "A,c\nd,f\n");
            let _ = s.take_notes(p).unwrap();
            let d = s.drop_deleted(p);
            let t = table_of(&s, p);
            println!("drop {d:?}; cols {:?} cells {:?}", t.live_cols(), t.cells);
            assert!(d.is_ok(), "{d:?}");
            let stray: Vec<_> = t
                .cells
                .keys()
                .filter(|(r, c)| !t.rows.knows(r) || !t.cols.knows(c))
                .collect();
            assert!(stray.is_empty(), "{stray:?}");
            // the restore form, on a fresh doc
            let f = Fix::new();
            std::fs::write(f.file(), "a,b,c\nd,e,f\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c3", "T");
            op(&s, p, &EditOptions::new(), |t| t.delete_col(&k("c3"))).unwrap();
            f.save(&s, "A,c\nd,f\n");
            let r = op(&s, p, &EditOptions::new(), |t| t.restore_col(&k("c3")));
            println!("restore {r:?} -> {:?}", rows(&s, p));
            assert!(r.is_ok(), "{r:?}");
        }

        /// K2 Engine's item 2 (`renders_the_same_is_the_same_row`), literal: open `\nk,k\n` (r1 a
        /// blank line); p2 sets r1/c1 := q; the agent reads V (`q,\nk,k\n`, r1/c2 absent); p2
        /// undoes it and sets r1/c2 := z (live `,z\nk,k\n`, r1/c1 absent); the agent writes
        /// `,z\nk,k\n` at V. Rows that render the same are the same row: `write --base` applies
        /// clean; the outside-save path (`write_live_grid` from V) notes nothing.
        fn engine_renders_the_same(outside: bool) {
            let f = Fix::new();
            let (s, p2, a) = with_bot(&f, "\nk,k\n");
            set(&s, p2, "r1", "c1", "q");
            let v = s.read(a).unwrap();
            let cells = |s: &Session| {
                let t = table_of(s, p2);
                t.live_rows()
                    .iter()
                    .map(|r| {
                        t.live_cols()
                            .iter()
                            .map(|c| t.cell(r, c).map(str::to_owned))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            };
            let some = |x: &str| Some(x.to_owned());
            assert_eq!(String::from_utf8(v.bytes.clone()).unwrap(), "q,\nk,k\n");
            assert_eq!(
                cells(&s),
                [vec![some("q"), None], vec![some("k"), some("k")]]
            );
            assert!(matches!(
                s.undo(p2, 1).unwrap(),
                UndoOutcome::Applied { .. }
            ));
            set(&s, p2, "r1", "c2", "z");
            assert_eq!(text(&s, p2), ",z\nk,k\n");
            assert_eq!(
                cells(&s),
                [vec![None, some("z")], vec![some("k"), some("k")]]
            );
            let body = b",z\nk,k\n";
            let r = if outside {
                s.write_live_grid(a, v.version, body, &GridWriteOptions::default())
            } else {
                s.write_base_grid(a, v.version, body, &GridWriteOptions::default())
            }
            .unwrap();
            let n = cells_of(&overwritten(&s.take_notes(p2).unwrap()));
            let t = table_of(&s, p2);
            println!(
                "outside={outside}: {r:?} -> rows {:?} keys {:?} cells {:?}; notes {n:?}; audit {:?}",
                rows(&s, p2),
                t.live_rows(),
                t.cells,
                f.overrides()
            );
            assert!(
                matches!(r.outcome, GridWrite::Applied(_)) && r.skipped.is_empty(),
                "{r:?}"
            );
            assert_eq!(text(&s, p2), ",z\nk,k\n");
            assert!(n.is_empty(), "no note: {n:?}");
            assert!(
                f.overrides().is_empty(),
                "no audit override: {:?}",
                f.overrides()
            );
        }

        #[test]
        fn engine_renders_the_same_is_the_same_row_write_base() {
            engine_renders_the_same(false);
        }

        #[test]
        fn engine_renders_the_same_is_the_same_row_outside_save() {
            engine_renders_the_same(true);
        }

        /// K2 Engine (8,372 cases): base r2 blank; the live side sets r2/c1 := `""`; the file saves
        /// r2 as `,a`. The file's row wins; its c1 is absent but renders `""`, so the live value
        /// already stands in the result: no note.
        #[test]
        fn a_live_empty_the_files_row_renders_anyway_is_not_noted() {
            let f = Fix::new();
            std::fs::write(f.file(), "x,y\n\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            println!("V rows {:?} cells {:?}", rows(&s, p), table_of(&s, p).cells);
            set(&s, p, "r2", "c1", "");
            println!("live rows {:?}", rows(&s, p));
            f.save(&s, "x,y\n,a\n");
            let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
            let t = table_of(&s, p);
            println!(
                "after the save: rows {:?} r2 {:?} {:?}; notes {n:?}",
                rows(&s, p),
                t.cell(&k("r2"), &k("c1")),
                t.cell(&k("r2"), &k("c2"))
            );
            assert_eq!(rows(&s, p)[1], "r2=,a");
            assert!(n.is_empty(), "no note: {n:?}");
        }

        /// `HiddenCellWrite::message` (csv contract §4.3's template), golden: the row form, the
        /// column form, both, and a table with a header (the column still named by key: a column
        /// name is content). `tests/golden/hidden_writes.txt`; `MPC_BLESS=1` rewrites it.
        #[test]
        fn hidden_write_messages_golden() {
            let raw = |s: &Session, c: ConnId, r: &str, col: &str| {
                let res = s
                    .edit_ops(
                        c,
                        vec![Edit::Map {
                            root: CELLS.into(),
                            ops: vec![MapOp::Set {
                                key: table::cell_key(&k(r), &k(col)),
                                value: Value::str("SECRET"),
                            }],
                        }],
                        &EditOptions::new(),
                    )
                    .unwrap();
                assert_eq!(res.hidden.len(), 1, "{res:?}");
                let m = res.hidden[0].message.clone();
                assert!(!m.contains("SECRET"), "no value in the message: {m}");
                m
            };
            let mut got = String::new();
            for (label, header) in [
                ("row", false),
                ("col", false),
                ("both", false),
                ("row, header on", true),
            ] {
                let f = Fix::new();
                std::fs::write(f.file(), "name,city\nAna,Oslo\nBen,Rome\n").unwrap();
                let s = f.open();
                // (the fixture's participants: p1, p2 and bot; bot deletes the column)
                let (p1, p2, p3) = (f.join(&s, "p1"), f.join(&s, "p2"), f.join(&s, "bot"));
                if header {
                    op(&s, p1, &EditOptions::new(), |t| t.set_header(true)).unwrap();
                    assert!(!table_of(&s, p1).cells.values().any(|v| v == "SECRET"));
                }
                if label.starts_with("row") || label == "both" {
                    op(&s, p1, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
                }
                if label == "col" || label == "both" {
                    op(&s, p3, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
                }
                let (r, c) = match label {
                    "col" => ("r1", "c2"),
                    "both" => ("r2", "c2"),
                    _ => ("r2", "c1"),
                };
                let m = raw(&s, p2, r, c);
                let name = if header { "name" } else { "" };
                assert!(name.is_empty() || !m.contains(name), "no column name: {m}");
                got.push_str(&format!("{label}: {m}\n"));
            }
            println!("{got}");
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/golden/hidden_writes.txt");
            if std::env::var_os("MPC_BLESS").is_some() {
                std::fs::write(&path, &got).unwrap();
                return;
            }
            let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
                panic!("{} is missing (MPC_BLESS=1 writes it)", path.display())
            });
            assert_eq!(
                got,
                want,
                "{} differs (MPC_BLESS=1 regenerates it)",
                path.display()
            );
        }

        /// K2 Engine's final verdict (12 cases): base `,a`; the live side switches r1/c1 between
        /// `""` and absent (byte-neutral, `neutral_live`) or really changes it (the control); the
        /// text writes `q,a` at V. A byte-neutral switch is no live edit of the cell, so
        /// `write --base` applies the text's `q` with no conflict; a real live change makes r1
        /// changed by both, one `edited_row` (§3.4).
        fn write_base_over_a_neutral_live_switch(live: &str) {
            under_merge_mutant(|| {
                let f = Fix::new();
                let (s, p, bot) = with_bot(&f, ",a\n");
                let unset = |s: &Session| {
                    s.edit_ops(
                        p,
                        vec![Edit::Map {
                            root: CELLS.into(),
                            ops: vec![MapOp::Delete {
                                key: table::cell_key(&k("r1"), &k("c1")),
                            }],
                        }],
                        &EditOptions::new(),
                    )
                    .unwrap();
                };
                // the reverse starts from an absent c1 (the same bytes) at V
                if live == "absent to empty" {
                    unset(&s);
                }
                let base = s.read(bot).unwrap().version;
                match live {
                    "empty to absent" => unset(&s),
                    "absent to empty" => set(&s, p, "r1", "c1", ""),
                    _ => set(&s, p, "r1", "c1", "x"),
                }
                assert_eq!(
                    rows(&s, p),
                    [if live == "a real change" {
                        "r1=x,a"
                    } else {
                        "r1=,a"
                    }]
                );
                let r = s
                    .write_base_grid(bot, base, b"q,a\n", &GridWriteOptions::default())
                    .unwrap();
                println!("{live}: {r:?} -> {:?}", rows(&s, p));
                if live == "a real change" {
                    let GridWrite::Conflicts(c) = &r.outcome else {
                        panic!("{live}: {r:?}")
                    };
                    // both changed r1 (a real live edit of c1, the text's q): the row is merged
                    // whole, one `edited_row` naming the live write (§3.4)
                    let at: Vec<_> = c
                        .iter()
                        .map(|c| {
                            (
                                c.conflict.rule.as_str(),
                                c.conflict.at.clone(),
                                c.conflict.theirs_writes.clone(),
                            )
                        })
                        .collect();
                    assert_eq!(
                        at,
                        [(
                            "edited_row",
                            table::Region::Row(k("r1")),
                            vec![table::Region::Cell {
                                row: k("r1"),
                                col: k("c1")
                            }]
                        )],
                        "{c:?}"
                    );
                    assert_eq!(c[0].written_by.as_deref(), Some("p1"), "{c:?}");
                    assert_eq!(rows(&s, p), ["r1=x,a"], "nothing applied");
                } else {
                    assert!(
                        matches!(r.outcome, GridWrite::Applied(_)) && r.skipped.is_empty(),
                        "{live}: {r:?}"
                    );
                    assert_eq!(rows(&s, p), ["r1=q,a"], "{live}");
                }
            })
        }

        #[test]
        fn write_base_over_a_live_empty_to_absent_switch_applies() {
            write_base_over_a_neutral_live_switch("empty to absent");
        }

        #[test]
        fn write_base_over_a_live_absent_to_empty_switch_applies() {
            write_base_over_a_neutral_live_switch("absent to empty");
        }

        #[test]
        fn write_base_over_a_real_live_change_of_the_cell_conflicts() {
            write_base_over_a_neutral_live_switch("a real change");
        }

        /// K2 Engine's stated gap, column rename conflicts, at the session: the live side renames
        /// c2, a text renames it otherwise. `write --base`: a `name` conflict, nothing applied; an
        /// outside save: the file's name, the live name noted to its author.
        #[test]
        fn a_column_renamed_on_both_sides() {
            for outside in [false, true] {
                let f = Fix::new();
                let (s, p, bot) = with_bot(&f, "n,city\na,b\n");
                op(&s, p, &EditOptions::new(), |t| t.set_header(true)).unwrap();
                f.advance(2_000);
                s.flush(p).unwrap();
                let base = s.read(bot).unwrap().version;
                op(&s, p, &EditOptions::new(), |t| {
                    t.rename_col(&k("c2"), "town")
                })
                .unwrap();
                let body = "n,place\na,b\n";
                if outside {
                    f.save(&s, body);
                    let t = table_of(&s, p);
                    let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
                    println!("outside: name {:?}; notes {n:?}", t.name(&k("c2")));
                    assert_eq!(t.name(&k("c2")), Some("place"), "the file's name");
                    assert!(
                        n.iter().any(|c| c.2.contains("town")),
                        "the live name noted: {n:?}"
                    );
                } else {
                    let r = s
                        .write_base_grid(bot, base, body.as_bytes(), &GridWriteOptions::default())
                        .unwrap();
                    println!("write --base: {r:?}");
                    let GridWrite::Conflicts(c) = &r.outcome else {
                        panic!("{r:?}")
                    };
                    assert!(
                        c.iter().any(|c| c.conflict.rule.as_str() == "name"),
                        "{c:?}"
                    );
                    assert_eq!(
                        table_of(&s, p).name(&k("c2")),
                        Some("town"),
                        "nothing applied"
                    );
                }
            }
        }

        /// K2 Engine's stated gap, a hidden cell removed by a reconcile map delete: on a ragged doc
        /// the text shortens r1 (a cell delete at the same width), while the live side wrote T
        /// into that cell and then deleted r1. `write --base`: `deleted_row`, nothing applied; an
        /// outside save: the file's row comes back whole, T noted to its author.
        #[test]
        fn a_text_cell_delete_over_a_hidden_live_value() {
            for outside in [false, true] {
                let f = Fix::new();
                let (s, p, bot) = with_bot(&f, "a,b,c\nd,e,f\nx\n");
                let base = s.read(bot).unwrap().version;
                set(&s, p, "r1", "c3", "T");
                op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
                let body = "a,b\nd,e,f\nx\n";
                if outside {
                    f.save(&s, body);
                    let t = table_of(&s, p);
                    let n = cells_of(&overwritten(&s.take_notes(p).unwrap()));
                    println!(
                        "outside: rows {:?} cells {:?} notes {n:?}",
                        rows(&s, p),
                        t.cells
                    );
                    assert!(t.rows.is_live(&k("r1")), "the file's row comes back");
                    assert_eq!(t.cell(&k("r1"), &k("c3")), None, "the file's row, whole");
                    assert!(
                        n.iter().any(|c| c.2.split(", ").any(|x| x == "\"T\"")),
                        "T noted: {n:?}"
                    );
                } else {
                    let r = s
                        .write_base_grid(bot, base, body.as_bytes(), &GridWriteOptions::default())
                        .unwrap();
                    println!("write --base: {r:?}");
                    let GridWrite::Conflicts(c) = &r.outcome else {
                        panic!("{r:?}")
                    };
                    assert!(
                        c.iter().any(|c| c.conflict.rule.as_str() == "deleted_row"),
                        "{c:?}"
                    );
                    assert_eq!(
                        table_of(&s, p).cell(&k("r1"), &k("c3")),
                        Some("T"),
                        "kept, hidden"
                    );
                }
            }
        }

        /// K2 Engine's stated gap, a hidden cell removed by a revert's cell delete: p1 sets r1/c1 :=
        /// A (into an absent cell), p2 overwrites it with B and deletes r1; p1 reverts its change.
        /// The revert would remove the cell, which p2 wrote since: it conflicts (`written since`),
        /// and B stays, hidden.
        #[test]
        fn a_revert_over_a_hidden_value_written_since_conflicts() {
            let f = Fix::new();
            std::fs::write(f.file(), ",x\nd,e\n").unwrap();
            let s = f.open();
            let (p1, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            s.edit_ops(
                p1,
                vec![Edit::Map {
                    root: CELLS.into(),
                    ops: vec![MapOp::Delete {
                        key: table::cell_key(&k("r1"), &k("c1")),
                    }],
                }],
                &EditOptions::new(),
            )
            .unwrap();
            let change = op(&s, p1, &EditOptions::new(), |t| {
                t.set_cell(&k("r1"), &k("c1"), "A")
            })
            .unwrap();
            set(&s, p2, "r1", "c1", "B");
            op(&s, p2, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
            let out = s.revert(p1, change, false).unwrap();
            let t = table_of(&s, p1);
            println!("revert: {out:?}; r1/c1 {:?}", t.cell(&k("r1"), &k("c1")));
            let UndoOutcome::Conflicts { conflicts, .. } = &out else {
                panic!("{out:?}")
            };
            assert!(
                conflicts
                    .iter()
                    .any(|c| c.why.as_deref() == Some("written since")),
                "{conflicts:?}"
            );
            assert_eq!(t.cell(&k("r1"), &k("c1")), Some("B"), "B kept, hidden");
        }

        /// K2 Engine's 12 cases (its session diff 7ad89c88): doc `\nk,k\n` (r1 blank); p2 sets
        /// r1/c2 := a; a version is named; p2 sets r1/c1 := `""` (V: r1 `,a`, c1 present `""`); a
        /// live step takes c1 back to absent (p2's undo, p2's revert of that change, or p1's
        /// restore of the named version), so r1 still renders `,a`; the agent writes
        /// `<row>\nk,k\n` at V. Each is a byte-neutral live switch of c1 (`neutral_live`), so
        /// `write --base` applies the text's row, no conflict, nothing skipped.
        #[test]
        fn write_base_after_a_live_step_back_to_absent_applies_the_text() {
            under_merge_mutant(|| {
                let mut failed = Vec::new();
                for step in ["undo", "revert", "restore"] {
                    for row in ["q,a", "q,", "q,z", "b,a"] {
                        let why = format!("{step} x {row:?}");
                        let f = Fix::new();
                        let (s, p1, bot) = with_bot(&f, "\nk,k\n");
                        let p2 = f.join(&s, "p2");
                        set(&s, p2, "r1", "c2", "a");
                        f.advance(10_000);
                        let named = s.name_version(p1, "before", None).unwrap();
                        f.advance(10_000);
                        let change = op(&s, p2, &EditOptions::new(), |t| {
                            t.set_cell(&k("r1"), &k("c1"), "")
                        })
                        .unwrap();
                        let v = s.read(bot).unwrap().version;
                        assert_eq!(
                            table_of(&s, p1).cell(&k("r1"), &k("c1")),
                            Some(""),
                            "{why}: at V"
                        );
                        assert_eq!(rows(&s, p1)[0], "r1=,a", "{why}: at V");
                        f.advance(10_000);
                        match step {
                            "undo" => {
                                let u = s.undo(p2, 1).unwrap();
                                assert!(matches!(u, UndoOutcome::Applied { .. }), "{why}: {u:?}");
                            }
                            "revert" => {
                                let u = s.revert(p2, change, false).unwrap();
                                assert!(matches!(u, UndoOutcome::Applied { .. }), "{why}: {u:?}");
                            }
                            _ => {
                                s.restore(p1, named).unwrap();
                            }
                        }
                        let t = table_of(&s, p1);
                        assert_eq!(
                            t.cell(&k("r1"), &k("c1")),
                            None,
                            "{why}: c1 absent after the step"
                        );
                        assert_eq!(rows(&s, p1)[0], "r1=,a", "{why}: r1 still renders ,a");
                        let body = format!("{row}\nk,k\n");
                        let r = s
                            .write_base_grid(bot, v, body.as_bytes(), &GridWriteOptions::default())
                            .unwrap();
                        let got = rows(&s, p1);
                        println!("{why}: {:?} -> {got:?}", r.outcome);
                        let ok = matches!(r.outcome, GridWrite::Applied(_))
                            && r.skipped.is_empty()
                            && got[0] == format!("r1={row}")
                            && got[1] == "r2=k,k";
                        if !ok {
                            failed.push(format!("{why}: {r:?} -> {got:?}"));
                        }
                    }
                }
                assert!(
                    failed.is_empty(),
                    "{} of 12 failed:\n{}",
                    failed.len(),
                    failed.join("\n")
                );
            })
        }

        /// The grid-trace revert `Err` (seeds 4168, 9897, 1655 and six more): a text write keeps a
        /// row on its key without leaving it exact (S13 marks it), someone deletes the row, the owner
        /// drops it, and the text's writer reverts the write. S13 rewrites a marked row whole
        /// (`whole_row_back`), which would restore the row: but the row is dropped (S11). The
        /// reversal must give an outcome (`dropped`), never an engine refusal.
        fn reversal_of_a_text_over_a_dropped_marked_row(verb: &str) {
            under_merge_mutant(|| {
                let f = Fix::new();
                let (s, p, bot) = with_bot(&f, "a,b\nc,d\n");
                let base = s.read(bot).unwrap().version;
                let r = s
                    .write_base_grid(bot, base, b"A,b\nc,d\n", &GridWriteOptions::default())
                    .unwrap();
                let GridWrite::Applied(res) = &r.outcome else {
                    panic!("{r:?}")
                };
                let change = res.version;
                // the redo case: the writer undoes the text first, then the row is deleted and dropped
                if verb.starts_with("redo") {
                    let u = s.undo(bot, 1).unwrap();
                    assert!(matches!(u, UndoOutcome::Applied { .. }), "{u:?}");
                }
                op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
                s.drop_deleted(p).unwrap();
                assert!(!table_of(&s, p).rows.knows(&k("r1")), "r1 dropped");
                let out = match verb {
                    "revert" => s.revert(bot, change, false),
                    "revert --partial" => s.revert(bot, change, true),
                    "undo" => s.undo(bot, 1),
                    "redo" => s.redo(bot, 1),
                    "redo --partial" => {
                        s.redo_with(bot, &UndoRequest::steps(1).mode(UndoMode::Partial))
                    }
                    _ => s.undo_with(bot, &UndoRequest::steps(1).mode(UndoMode::Partial)),
                };
                println!("{verb}: {out:?} -> {:?}", rows(&s, p));
                let out = out.unwrap_or_else(|e| panic!("{verb}: an error, not an outcome: {e:?}"));
                let named_dropped = |c: &[k2_mpc_base::session::ConflictRegion]| {
                    c.iter().any(|x| x.why.as_deref() == Some("dropped"))
                };
                match &out {
                    UndoOutcome::Conflicts { conflicts, .. }
                    | UndoOutcome::Blocked { conflicts } => {
                        assert!(named_dropped(conflicts), "{verb}: {conflicts:?}");
                    }
                    UndoOutcome::Applied { skipped, .. } => {
                        assert!(
                            named_dropped(skipped),
                            "{verb}: the row skipped as dropped: {out:?}"
                        );
                    }
                    o => panic!("{verb}: {o:?}"),
                }
                assert!(
                    !table_of(&s, p).rows.knows(&k("r1")),
                    "{verb}: nothing revived"
                );
            })
        }

        #[test]
        fn a_revert_of_a_text_over_a_dropped_marked_row_is_dropped() {
            reversal_of_a_text_over_a_dropped_marked_row("revert");
        }

        #[test]
        fn a_partial_revert_of_a_text_over_a_dropped_marked_row_skips_it() {
            reversal_of_a_text_over_a_dropped_marked_row("revert --partial");
        }

        #[test]
        fn an_undo_of_a_text_over_a_dropped_marked_row_is_dropped() {
            reversal_of_a_text_over_a_dropped_marked_row("undo");
        }

        #[test]
        fn a_redo_of_a_text_over_a_dropped_marked_row_is_dropped() {
            reversal_of_a_text_over_a_dropped_marked_row("redo");
        }

        #[test]
        fn a_partial_redo_of_a_text_over_a_dropped_marked_row_skips_it() {
            reversal_of_a_text_over_a_dropped_marked_row("redo --partial");
        }

        #[test]
        fn a_partial_undo_of_a_text_over_a_dropped_marked_row_skips_it() {
            reversal_of_a_text_over_a_dropped_marked_row("undo --partial");
        }

        /// A table a,b/c,d/e,f with p1 and p2 joined, after `setup` by p1 (the owner): `row
        /// deleted` / `col deleted` (r2 / c2 deleted, still known), `row dropped` / `col dropped`
        /// (then `rows drop-deleted`: the version it made is returned), or `none`.
        fn keyed_case(setup: &str) -> (Fix, Arc<Session>, ConnId, ConnId, Option<u64>) {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\ne,f\n").unwrap();
            let s = f.open();
            let (p1, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            if setup.starts_with("row") {
                op(&s, p1, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
            } else if setup.starts_with("col") {
                op(&s, p1, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
            }
            let dropped = setup
                .ends_with("dropped")
                .then(|| s.drop_deleted(p1).unwrap().version);
            (f, s, p1, p2, dropped)
        }

        fn set_cell(r: &str, c: &str, v: &str) -> Edit {
            Edit::Map {
                root: CELLS.into(),
                ops: vec![MapOp::Set {
                    key: table::cell_key(&k(r), &k(c)),
                    value: Value::str(v),
                }],
            }
        }

        /// The doc's own cell map (the read's content: every key the doc holds, orphans
        /// included, not the table view).
        fn cell_map(s: &Session, c: ConnId) -> std::collections::BTreeMap<String, Value> {
            s.read(c)
                .unwrap()
                .content
                .into_iter()
                .find_map(|(root, n)| match n {
                    k2_mpc_base::doc::Neutral::Map(m) if root == CELLS => Some(m),
                    _ => None,
                })
                .unwrap()
        }

        /// p2's raw keyed write `edits` must be refused whole as `MPC-E805 no_element` (exit 4,
        /// the grid verbs' unknown key), naming `root`/`key` and the drop (`dropped`: its
        /// version and p1) or saying it was never in the table; nothing applies: the version,
        /// the doc's cell map (no orphan under the key) and the disk stay as they were.
        fn assert_refused(
            what: &str,
            setup: &str,
            edits: Vec<Edit>,
            root: &str,
            key: &str,
            cell: &str,
        ) {
            let (_f, s, _p1, p2, dropped) = keyed_case(setup);
            let before = s.read(p2).unwrap().version;
            let cells_before = cell_map(&s, p2);
            let w = s.edit_ops(p2, edits, &EditOptions::new());
            let e = match w {
                Err(e) => e,
                Ok(r) => panic!(
                    "{what}: a keyed write under {root} {key} applied (version {}), hidden {:?}",
                    r.version, r.hidden
                ),
            };
            let r = k2_mpc_base::errors::Refusal::report(&e);
            assert_eq!((r.id(), r.exit()), ("MPC-E805", 4), "{what}: {e}");
            let msg = e.to_string();
            assert!(msg.starts_with(&format!("{root}: {key} ")), "{what}: {msg}");
            match dropped {
                Some(v) => {
                    assert!(
                        msg.contains("was dropped")
                            && msg.contains("by p1")
                            && msg.contains(&format!("change {v}")),
                        "{what}: {msg}"
                    );
                    assert!(!msg.contains("restore"), "{what}: {msg}");
                }
                None => assert!(msg.contains("not in this table"), "{what}: {msg}"),
            }
            assert_eq!(
                s.read(p2).unwrap().version,
                before,
                "{what}: nothing applies"
            );
            let cells = cell_map(&s, p2);
            assert!(
                !cells.contains_key(cell),
                "{what}: a cell stored under {cell:?}"
            );
            assert_eq!(cells, cells_before, "{what}: the cell map");
            assert_eq!(s.audit_disk_sync().unwrap(), vec![], "{what}: K-AUD-1");
            assert!(s.audit().unwrap().is_clean(), "{what}: audit");
        }

        /// Ruling (A): a raw keyed cell write under a row the owner has dropped is refused
        /// whole (no orphan cell nobody could see, restore or sweep), naming the drop.
        #[test]
        fn a_keyed_write_under_a_dropped_row_is_refused() {
            under_merge_mutant(|| {
                let cell = table::cell_key(&k("r2"), &k("c1"));
                assert_refused(
                    "row",
                    "row dropped",
                    vec![set_cell("r2", "c1", "z")],
                    "rows",
                    "r2",
                    &cell,
                );
            });
        }

        #[test]
        fn a_keyed_write_under_a_dropped_column_is_refused() {
            under_merge_mutant(|| {
                let cell = table::cell_key(&k("r1"), &k("c2"));
                assert_refused(
                    "col",
                    "col dropped",
                    vec![set_cell("r1", "c2", "z")],
                    "cols",
                    "c2",
                    &cell,
                );
            });
        }

        /// A key never in the table (r99, c99) is refused the same way, saying so.
        #[test]
        fn a_keyed_write_under_a_never_existing_row_or_column_is_refused() {
            under_merge_mutant(|| {
                let cell = table::cell_key(&k("r99"), &k("c1"));
                assert_refused(
                    "r99",
                    "none",
                    vec![set_cell("r99", "c1", "z")],
                    "rows",
                    "r99",
                    &cell,
                );
                let cell = table::cell_key(&k("r1"), &k("c99"));
                assert_refused(
                    "c99",
                    "none",
                    vec![set_cell("r1", "c99", "z")],
                    "cols",
                    "c99",
                    &cell,
                );
            });
        }

        /// `edit_ops` has no partial mode: one bad op refuses the whole edit. A good cell write
        /// (r1/c1) beside a write under a dropped row applies nothing.
        #[test]
        fn a_keyed_edit_with_one_write_under_a_dropped_key_is_refused_whole() {
            under_merge_mutant(|| {
                let cell = table::cell_key(&k("r2"), &k("c1"));
                assert_refused(
                    "batch",
                    "row dropped",
                    vec![set_cell("r1", "c1", "ok"), set_cell("r2", "c1", "z")],
                    "rows",
                    "r2",
                    &cell,
                );
            });
        }

        /// K2 Cores' ruling (Q1): a raw keyed cell *delete* under a row the table doesn't know
        /// (dropped, or never in it) stays accepted as a no-op: `Ok`, nothing stored under the
        /// key (the doc's own cell map unchanged), no cell change counted, no hidden-write
        /// message and no note. Pins today's behaviour.
        #[test]
        fn a_keyed_delete_under_an_unknown_row_is_a_no_op() {
            under_merge_mutant(|| {
                for (what, setup, r) in [("dropped", "row dropped", "r2"), ("never", "none", "r99")]
                {
                    let (_f, s, _p1, p2, _) = keyed_case(setup);
                    let cell = table::cell_key(&k(r), &k("c1"));
                    let before = s.read(p2).unwrap().version;
                    let cells_before = cell_map(&s, p2);
                    let _ = s.take_notes(p2).unwrap();
                    let w = s
                        .edit_ops(
                            p2,
                            vec![Edit::Map {
                                root: CELLS.into(),
                                ops: vec![MapOp::Delete { key: cell.clone() }],
                            }],
                            &EditOptions::new(),
                        )
                        .unwrap_or_else(|e| panic!("{what}: {e}"));
                    assert!(w.hidden.is_empty(), "{what}: {:?}", w.hidden);
                    assert!(
                        w.elems
                            .iter()
                            .filter(|c| c.root == CELLS)
                            .all(|c| c.set == 0 && c.deleted == 0),
                        "{what}: {:?}",
                        w.elems
                    );
                    assert_eq!(w.version, before, "{what}: nothing applied");
                    assert_eq!(s.read(p2).unwrap().version, before, "{what}");
                    let cells = cell_map(&s, p2);
                    assert!(!cells.contains_key(&cell), "{what}: {cell:?} stored");
                    assert_eq!(cells, cells_before, "{what}: the cell map");
                    assert!(s.take_notes(p2).unwrap().notes.is_empty(), "{what}: a note");
                    assert_eq!(s.audit_disk_sync().unwrap(), vec![], "{what}: K-AUD-1");
                    assert!(s.audit().unwrap().is_clean(), "{what}: audit");
                }
            });
        }

        /// The other side of the line: a deleted row or column the table still knows takes
        /// the write as a hidden write with its §4.3 message (K-CSV-4), and a key the same edit
        /// inserts first takes it as a plain write.
        #[test]
        fn a_keyed_write_under_a_deleted_or_newly_inserted_key_still_applies() {
            under_merge_mutant(|| {
                for (what, r, c, restore) in [
                    (
                        "row deleted",
                        "r2",
                        "c1",
                        "row restore --row r2 brings it back",
                    ),
                    (
                        "col deleted",
                        "r1",
                        "c2",
                        "col restore --col c2 brings it back",
                    ),
                ] {
                    let (_f, s, _p1, p2, _) = keyed_case(what);
                    let w = s
                        .edit_ops(p2, vec![set_cell(r, c, "z")], &EditOptions::new())
                        .unwrap_or_else(|e| panic!("{what}: {e}"));
                    assert_eq!(w.hidden.len(), 1, "{what}: {:?}", w.hidden);
                    assert_eq!(
                        w.hidden[0].message,
                        format!(
                            "{r}: {c} set by p2 in a {} deleted by p1; {restore}",
                            if what.starts_with("row") {
                                "row"
                            } else {
                                "column"
                            }
                        ),
                        "{what}"
                    );
                    let cell = table::cell_key(&k(r), &k(c));
                    assert_eq!(
                        cell_map(&s, p2).get(&cell),
                        Some(&Value::str("z")),
                        "{what}"
                    );
                }
                for (what, axis, r, c) in
                    [("row", ROWS, "r9", "c1"), ("col", table::COLS, "r1", "c9")]
                {
                    let (_f, s, _p1, p2, _) = keyed_case("none");
                    let new = if what == "row" { r } else { c };
                    let edits = vec![
                        Edit::List {
                            root: axis.into(),
                            ops: vec![ListOp::Insert {
                                key: k(new),
                                at: Anchor::End,
                                value: Value::Null,
                            }],
                        },
                        set_cell(r, c, "z"),
                    ];
                    let w = s
                        .edit_ops(p2, edits, &EditOptions::new())
                        .unwrap_or_else(|e| panic!("{what} inserted: {e}"));
                    assert!(w.hidden.is_empty(), "{what} inserted: {:?}", w.hidden);
                    let cell = table::cell_key(&k(r), &k(c));
                    assert_eq!(
                        cell_map(&s, p2).get(&cell),
                        Some(&Value::str("z")),
                        "{what} inserted"
                    );
                }
            });
        }

        /// K2 Engine Q1: a drop's notes are stored with the drop (one store transaction). p2
        /// writes a cell, the owner deletes its column and drops, the session restarts: p2's
        /// `dropped_col` note still arrives with the exact value, and the `BeforeDrop` version
        /// is there.
        #[test]
        fn a_drops_note_and_version_survive_a_restart() {
            use k2_mpc_base::store::VersionReason;
            let mut f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let (own, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            set(&s, p2, "r1", "c2", "COLV");
            op(&s, own, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
            s.drop_deleted(own).unwrap();
            drop(s);
            f.restart();
            let s = f.open();
            let (own, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            let n = overwritten(&s.take_notes(p2).unwrap());
            println!("after restart: notes to p2 {:?}", cells_of(&n));
            assert_eq!(n.len(), 1, "{n:?}");
            assert_eq!((n[0].rule, n[0].region.as_str()), ("dropped_col", "col c2"));
            assert_eq!(n[0].yours, "\"COLV\"");
            let v = s
                .versions(own)
                .unwrap()
                .into_iter()
                .find(|v| v.reason == VersionReason::BeforeDrop)
                .expect("a BeforeDrop version after the restart");
            let content = s.show_content(own, v.id).unwrap();
            let cells = content
                .iter()
                .find(|(r, _)| r == CELLS)
                .map(|(_, n)| format!("{n:?}"))
                .unwrap();
            assert!(cells.contains("COLV"), "{cells}");
        }

        /// P44: `rows drop-deleted` cuts a version first whose cell map holds the dropped rows' and
        /// columns' cells (no store format change: a version's cell map is the whole map).
        #[test]
        fn a_before_drop_version_holds_the_dropped_cells() {
            use k2_mpc_base::store::VersionReason;
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let (own, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            set(&s, p2, "r1", "c2", "COLV");
            set(&s, p2, "r2", "c1", "ROWV");
            op(&s, own, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
            op(&s, own, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
            s.drop_deleted(own).unwrap();
            let v = s
                .versions(own)
                .unwrap()
                .into_iter()
                .find(|v| v.reason == VersionReason::BeforeDrop)
                .expect("a BeforeDrop version");
            let content = s.show_content(own, v.id).unwrap();
            let cells = content
                .iter()
                .find(|(r, _)| r == CELLS)
                .map(|(_, n)| format!("{n:?}"))
                .unwrap();
            println!("BeforeDrop {} content: {content:?}", v.id);
            assert!(cells.contains("COLV") && cells.contains("ROWV"), "{cells}");
            // its lists hold live elements only: no position for the dropped ones
            let rows = content
                .iter()
                .find(|(r, _)| r == ROWS)
                .map(|(_, n)| format!("{n:?}"));
            assert!(!rows.unwrap().contains("r2"));
            // the drop's note carries the value, and names no version (v0.10.0)
            let n = overwritten(&s.take_notes(p2).unwrap());
            assert!(n.iter().all(|o| o.version.is_none()), "{n:?}");
        }

        /// S11: a partial revert of a drop writes nothing under a dropped key.
        #[test]
        fn a_partial_revert_of_a_drop_writes_nothing_under_a_dropped_key() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let (own, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            set(&s, p2, "r1", "c2", "COLV");
            set(&s, p2, "r2", "c1", "ROWV");
            op(&s, own, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
            op(&s, own, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
            let drop = s.drop_deleted(own).unwrap().version;
            let out = s.revert(own, drop, true).unwrap();
            println!("{out:?}");
            let t = table_of(&s, own);
            let dropped = |r: &Key, c: &Key| !t.rows.knows(r) || !t.cols.knows(c);
            let stray: Vec<_> = t.cells.keys().filter(|(r, c)| dropped(r, c)).collect();
            assert!(stray.is_empty(), "values under dropped keys: {stray:?}");
            assert!(!matches!(out, UndoOutcome::Applied { skipped, .. } if skipped.is_empty()));
        }

        /// PROBE (P44, S11): can the owner revert the drop itself, and does that bring the values back?
        #[test]
        fn probe_revert_of_a_drop() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let (own, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            set(&s, p2, "r1", "c2", "COLV");
            set(&s, p2, "r2", "c1", "ROWV");
            op(&s, own, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
            op(&s, own, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
            let drop = s.drop_deleted(own).unwrap().version;
            let out = s.revert(own, drop, false);
            println!("revert of the drop ({drop}): {out:?}");
            if let Ok(UndoOutcome::Conflicts { conflicts, .. }) = &out {
                for c in conflicts {
                    println!(
                        "  conflict kind {:?} why {:?} key {:?}",
                        c.kind, c.why, c.key
                    );
                }
            }
            let t = table_of(&s, own);
            println!(
                "after: rows {:?} cols {:?} values {:?}",
                t.live_rows(),
                t.live_cols(),
                t.cells.values().collect::<Vec<_>>()
            );
            let out = s.revert(own, drop, true);
            println!("revert --partial of the drop: {out:?}");
            let t = table_of(&s, own);
            println!(
                "after --partial: rows {:?} cols {:?} values {:?}",
                t.live_rows(),
                t.live_cols(),
                t.cells.values().collect::<Vec<_>>()
            );
        }

        /// PROBE (P44): does a version from before a drop hold the dropped values, and does a
        /// restore bring them back?
        #[test]
        fn probe_restore_after_a_drop() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let (own, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            set(&s, p2, "r1", "c2", "COLV");
            set(&s, p2, "r2", "c1", "ROWV");
            f.advance(10_000);
            op(&s, own, &EditOptions::new(), |t| t.delete_col(&k("c2"))).unwrap();
            op(&s, own, &EditOptions::new(), |t| t.delete_row(&k("r2"))).unwrap();
            let vs = s.versions(own).unwrap();
            println!(
                "versions before the drop: {:?}",
                vs.iter()
                    .map(|v| (v.id, v.seq, format!("{:?}", v.reason)))
                    .collect::<Vec<_>>()
            );
            let drop = s.drop_deleted(own).unwrap().version;
            for v in s.versions(own).unwrap() {
                let c = s.show_content(own, v.id).unwrap();
                let t = View::of_content(&c).table().unwrap();
                let holds: Vec<String> = t.cells.values().cloned().collect();
                println!("version {} seq {} {:?}: {:?}", v.id, v.seq, v.reason, holds);
            }
            println!("drop change {drop}");
            // restore the newest version that holds both
            let best = s
                .versions(own)
                .unwrap()
                .into_iter()
                .filter(|v| {
                    let c = s.show_content(own, v.id).unwrap();
                    let t = View::of_content(&c).table().unwrap();
                    t.cells.values().any(|x| x == "COLV") && t.cells.values().any(|x| x == "ROWV")
                })
                .max_by_key(|v| v.seq);
            println!(
                "a version holding both: {:?}",
                best.as_ref().map(|v| (v.id, v.seq))
            );
            if let Some(v) = best {
                s.restore(own, v.id).unwrap();
                let t = table_of(&s, own);
                println!(
                    "after the restore: {:?} {:?}",
                    rows(&s, own),
                    t.cells.values().collect::<Vec<_>>()
                );
            }
        }

        /// A version restore is a whole rewrite: the restored row wins, the person whose live
        /// cell it took gets the note, and the version cut before it holds that cell.
        #[test]
        fn a_version_restore_wins_whole_with_a_note() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            let own = f.join(&s, "p2");
            let v = s.name_version(own, "before", None).unwrap();
            set(&s, own, "r2", "c2", "OTHER");
            set(&s, p, "r2", "c1", "LIVE");
            s.restore(own, v).unwrap();
            assert_eq!(rows(&s, p), ["r1=a,b", "r2=c,d"], "no cell merged");
            let ow = overwritten(&s.take_notes(p).unwrap());
            assert_eq!(ow.len(), 1, "{ow:?}");
            assert_eq!(
                (ow[0].source, ow[0].region.as_str()),
                ("a restore", "r2/c1")
            );
            let vid = ow[0].version.expect("a version");
            assert_eq!(
                version_holds(&s, p, vid, "r2", "c1").as_deref(),
                Some("LIVE")
            );
            assert!(
                overwritten(&s.take_notes(own).unwrap()).is_empty(),
                "the restorer's own"
            );
        }

        /// A note names the row by line and shows the value, not only by key: the header switch
        /// here gave the row a new key, and its author only ever saw the old one.
        #[test]
        fn a_note_locates_a_row_whose_key_changed() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b,c\nd,e,f\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r1", "c3", "LIVE");
            op(&s, p, &EditOptions::new(), |t| t.set_header(true)).unwrap();
            op(&s, p, &EditOptions::new(), |t| t.set_header(false)).unwrap();
            f.save(&s, "a,FILE,c\nd,e,f\n");
            let ow = overwritten(&s.take_notes(p).unwrap());
            let line = ow[0].to_string();
            assert_eq!((ow[0].line, ow[0].col), (Some(1), Some(3)));
            assert!(
                line.contains("(L1, col 3)") && line.contains("\"LIVE\""),
                "{line}"
            );
        }

        /// A whole-file requote with live edits pending keeps them all: no notes, no bounces.
        #[test]
        fn a_requote_is_no_change() {
            let t = "a,b\nc,d\n";
            let f = Fix::new();
            std::fs::write(f.file(), t).unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            let bot = s
                .join(participant("bot", ParticipantKind::Agent), Stay::Held)
                .unwrap()
                .conn;
            let old = s.read(bot).unwrap().version;
            set(&s, p, "r2", "c1", "LIVE");
            // the kit's rows as the file re-reads them, every field quoted
            let mut base = table_of(&s, p);
            let mut recs = base.render();
            for r in &mut recs {
                for f in r.iter_mut() {
                    f.quote = Some(table::Quote::Quoted);
                }
            }
            recs[1][0].value = "c".into();
            let _ = &mut base;
            let tb = {
                let mut b = base.clone();
                b.set_cell(&k("r2"), &k("c1"), "c").unwrap();
                b
            };
            let r = table::reconcile(&tb, &recs).unwrap();
            assert_eq!(r.exact_rows, [k("r1"), k("r2")], "quoting is no change");
            // no mark either: a stale keyed write isn't bounced
            let stale = EditOptions::new().base(old);
            assert!(op(&s, bot, &stale, |t| t.set_cell(&k("r1"), &k("c1"), "x")).is_ok());
        }

        /// A connected session subscribed to the roster (what a person's editor and the JS
        /// client read) hears that an outside save overwrote their cell.
        #[test]
        fn the_overwritten_note_reaches_a_roster_subscriber() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r2", "c2", "LIVE");
            let rx = s.subscribe_roster(p).unwrap();
            f.save(&s, "a,b\nc,FILE\n");
            f.advance(1_000);
            s.flush_roster().unwrap();
            let got: Vec<RosterEvent> = rx.try_iter().collect();
            println!("roster: {got:?}");
            assert!(
                got.iter().any(
                    |e| matches!(e, RosterEvent::Overwritten { to, .. } if to.pid() == Some("p1"))
                ),
                "no Overwritten event among {} roster events",
                got.len()
            );
        }

        fn overwrite_pushes(got: &[RosterEvent]) -> Vec<&Overwrite> {
            got.iter()
                .filter_map(|e| match e {
                    RosterEvent::Overwritten { overwrite, .. } => Some(overwrite),
                    _ => None,
                })
                .collect()
        }

        /// K2 Cores' ruling (a): a purge reaches a grid's pushes that are queued but not yet
        /// sent, as it does a text doc's (`withhold_notes`).
        ///
        /// Steps:
        /// 1. An outside save overwrites p1's live value, which queues a push to p1.
        /// 2. Before the roster delivers it, an owner purges the value.
        /// 3. The push p1 then gets is withheld, with its values empty.
        ///
        /// Like a text doc's withheld note, it names no version: no `version`, no
        /// `restore_from`, and no version or restore hint in its line. The note p1 takes is the
        /// same, and nothing else carries the value.
        #[test]
        fn a_purge_withholds_a_grid_push_not_yet_sent() {
            let f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let (p1, p2) = (f.join(&s, "p1"), f.join(&s, "p2"));
            set(&s, p1, "r2", "c2", "SECRETLIVE");
            let rx = s.subscribe_roster(p1).unwrap();
            // a delivery now, so the next push waits for the roster interval
            f.advance(1_000);
            s.flush_roster().unwrap();
            let _ = rx.try_iter().count();
            // the outside save, inside the interval: queued, not sent
            std::fs::write(f.file(), "a,b\nc,FILE\n").unwrap();
            s.file_changed().unwrap();
            let early: Vec<RosterEvent> = rx.try_iter().collect();
            assert!(
                overwrite_pushes(&early).is_empty(),
                "the push must still be queued for this test to mean anything: {early:?}"
            );
            // the purge, before delivery
            s.purge(
                p2,
                k2_mpc_base::session::PurgeRequest::new(k2_mpc_base::session::PurgeScope::Text(
                    "SECRETLIVE".into(),
                )),
            )
            .unwrap();
            f.advance(1_000);
            s.flush_roster().unwrap();
            let got: Vec<RosterEvent> = rx.try_iter().collect();
            let pushes = overwrite_pushes(&got);
            assert_eq!(pushes.len(), 1, "{got:?}");
            let taken = overwritten(&s.take_notes(p1).unwrap());
            assert_eq!(taken.len(), 1, "the note p1 takes");
            for (what, o) in [("push", pushes[0]), ("take_notes", &taken[0])] {
                assert!(o.withheld, "{what}: {o:?}");
                assert!(o.yours.is_empty() && o.file.is_empty(), "{what}: {o:?}");
                assert!(
                    o.also
                        .iter()
                        .all(|c| c.yours.is_empty() && c.file.is_empty()),
                    "{what}: {o:?}"
                );
                assert!(
                    o.version.is_none() && o.restore_from.is_none(),
                    "{what}: a withheld note names a version: {o:?}"
                );
                let line = o.to_string();
                assert!(
                    !line.contains("version") && !line.contains("restore"),
                    "{what}: {line}"
                );
            }
            let all = format!("{got:?} {taken:?}");
            assert!(!all.contains("SECRETLIVE"), "{all}");
        }

        /// K2 Cores' ruling (b): a grid reader without the history right gets no version in
        /// its notes, as on a text doc: `version` and `restore_from` are cleared and
        /// `no_history` is set. This holds on every path:
        /// - the roster push;
        /// - a new subscriber's snapshot;
        /// - the waiting note it takes.
        ///
        /// Readers: a host Person and an App viewer (the handles lens), both without history,
        /// and a control Person with history, who still gets the version.
        #[test]
        fn a_grid_reader_without_history_gets_no_version() {
            for (pid, kind, history) in [
                ("p1", ParticipantKind::Person, false),
                ("widget", ParticipantKind::App, false),
                ("p2", ParticipantKind::Person, true),
            ] {
                let f = Fix::new();
                f.host.rights.lock().unwrap().insert(
                    pid.into(),
                    if history {
                        Rights::full()
                    } else {
                        Rights::editor()
                    },
                );
                std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
                let s = f.open();
                let c = s.join(participant(pid, kind), Stay::Held).unwrap().conn;
                set(&s, c, "r2", "c2", "LIVE");
                let rx = s.subscribe_roster(c).unwrap();
                f.save(&s, "a,b\nc,FILE\n");
                f.advance(1_000);
                s.flush_roster().unwrap();
                let got: Vec<RosterEvent> = rx.try_iter().collect();
                let check = |o: &Overwrite, path: &str| {
                    if history {
                        assert!(o.version.is_some() && !o.no_history, "{pid} {path}: {o:?}");
                    } else {
                        assert!(
                            o.version.is_none() && o.restore_from.is_none() && o.no_history,
                            "{pid} {path}: {o:?}"
                        );
                    }
                };
                let pushes = overwrite_pushes(&got);
                assert_eq!(pushes.len(), 1, "{pid}: {got:?}");
                check(pushes[0], "push");
                // a new subscriber's snapshot carries the waiting notes as the reader may have them
                let rx2 = s.subscribe_roster(c).unwrap();
                f.advance(1_000);
                s.flush_roster().unwrap();
                let mut snapshots = 0;
                for e in rx2.try_iter() {
                    if let RosterEvent::Snapshot { overwritten, .. } = e {
                        snapshots += 1;
                        for o in &overwritten {
                            check(o, "snapshot");
                        }
                    }
                }
                assert!(snapshots > 0, "{pid}: no snapshot");
                let taken = overwritten(&s.take_notes(c).unwrap());
                assert_eq!(taken.len(), 1, "{pid}");
                check(&taken[0], "take_notes");
            }
        }

        fn cat_notes(s: &Session, c: ConnId, verb: &str) -> Vec<Overwrite> {
            let argv: Vec<String> = verb.split(' ').map(str::to_owned).collect();
            let o: ToolOutput = s.run_tool(c, table::ROWS, &argv, b"").unwrap();
            overwritten(&o.notes)
        }

        /// An agent's (or anyone's) next verb carries the note, on a grid.
        #[test]
        fn the_overwritten_note_comes_with_the_next_verb_on_a_grid() {
            let mut wrong = Vec::new();
            for verb in ["cat", "head -n 1"] {
                let f = Fix::new();
                std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
                let s = f.open();
                let p = f.join(&s, "p1");
                set(&s, p, "r2", "c2", "LIVE");
                f.save(&s, "a,b\nc,FILE\n");
                let n = cat_notes(&s, p, verb);
                println!("next verb {verb}: {} note(s)", n.len());
                if n.is_empty() {
                    wrong.push(verb);
                }
            }
            assert!(wrong.is_empty(), "no note with {wrong:?}");
        }

        /// Every overridden live value gets its note: a save over more than the queue holds.
        #[test]
        fn every_overridden_cell_gets_a_note_past_the_queue_bound() {
            let n = 80;
            let body: String = (0..n).map(|i| format!("a{i},b{i}\n")).collect();
            let text: String = (0..n).map(|i| format!("a{i},F{i}\n")).collect();
            let f = Fix::new();
            std::fs::write(f.file(), &body).unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            for i in 1..=n {
                set(&s, p, &format!("r{i}"), "c2", "LIVE");
            }
            f.save(&s, &text);
            let got = overwritten(&s.take_notes(p).unwrap());
            let audit = f.overrides().len();
            let cells: usize = got.iter().map(Overwrite::cells).sum();
            println!(
                "{} notes for {cells} of {n} cells, audit {audit}",
                got.len()
            );
            assert_eq!(
                cells, n,
                "every overridden cell is in a note or the roll-up"
            );
            assert_eq!(audit, n);
            assert!(got.len() <= 64);
            let last = got.last().unwrap();
            assert!(last.is_rollup(), "{last:?}");
            assert!(last
                .to_string()
                .contains("more cells overwritten, see the log"));
        }

        /// The note survives the session being reloaded before the editor reads it.
        #[test]
        fn the_overwritten_note_survives_a_restart() {
            let mut f = Fix::new();
            std::fs::write(f.file(), "a,b\nc,d\n").unwrap();
            let s = f.open();
            let p = f.join(&s, "p1");
            set(&s, p, "r2", "c2", "LIVE");
            f.save(&s, "a,b\nc,FILE\n");
            drop(s);
            f.restart();
            let s = f.open();
            let p = f.join(&s, "p1");
            let got = overwritten(&s.take_notes(p).unwrap()).len();
            println!(
                "after restart: {got} note(s), audit {}",
                f.overrides().len()
            );
            assert_eq!(got, 1);
        }
    }
}

/// C1k (request 5): an edit closure reads the doc's codec settings from its view (the pack's
/// dialect, to hold an op to the codec's rules); a view the session didn't hand out has none.
#[test]
fn an_edit_closure_reads_the_codec_settings() {
    let f = fx("codec");
    let s = f.open("t.csv", "a,b\nc,d\n");
    let p = f.person(&s, "p");
    let mut seen = None;
    let r = s
        .edit_ops_with(p, &EditOptions::new(), |v| {
            seen = Some(v.codec().to_vec());
            Ok::<_, String>(Vec::new())
        })
        .unwrap();
    assert!(r.is_none(), "no edit");
    assert_eq!(seen.as_deref(), Some(&[1u8][..]), "csv-lite's settings");
    assert!(View::new(Default::default()).codec().is_empty());
    assert_eq!(
        View::of_content(&[]).with_codec(b"x").codec(),
        b"x",
        "set by the caller"
    );
}

/// C1k (request 7): the value limit is a knob (`Config::grid_max_value_len`, default 64 KiB),
/// raised or lowered by the host: an edit closure's table ops follow it (its view carries it),
/// and every structured change is held to it (`AtLimit`, `value bytes`, exit 2; raw ops too).
#[test]
fn the_value_limit_is_a_knob() {
    let set = |s: &Session, p: ConnId, v: String| {
        s.edit_ops_with(p, &EditOptions::new(), |view| {
            let mut t = view.table().map_err(|e| e.to_string())?;
            t.set_cell(&k("r1"), &k("c1"), &v)
                .map_err(|e| e.to_string())
        })
    };
    let raw = |s: &Session, p: ConnId, v: String| {
        s.edit_ops(
            p,
            vec![Edit::Map {
                root: CELLS.into(),
                ops: vec![MapOp::Set {
                    key: table::cell_key(&k("r1"), &k("c1")),
                    value: Value::str(&v),
                }],
            }],
            &EditOptions::new(),
        )
    };
    // the default
    let f = fx("value-default");
    let s = f.open("t.csv", "a,b\n");
    let p = f.person(&s, "p");
    assert_eq!(config().grid_max_value_len, 64 * 1024);
    assert_eq!(config().grid_max_value_len, table::DEFAULT_MAX_VALUE_LEN);
    set(&s, p, "x".repeat(64 * 1024)).unwrap();
    assert!(matches!(
        set(&s, p, "x".repeat(64 * 1024 + 1)),
        Err(k2_mpc_base::session::EditWithError::Compute(_))
    ));
    let e = raw(&s, p, "x".repeat(64 * 1024 + 1)).unwrap_err();
    assert!(
        matches!(
            e,
            SessionError::AtLimit {
                what: "value bytes",
                limit: 65536,
                would_be: 65537
            }
        ),
        "{e:?}"
    );
    assert_eq!(session_exit(&e), exit::USAGE);
    assert!(e.to_string().contains("65537 bytes"), "{e}");
    // lowered
    let mut c = config();
    c.grid_max_value_len = 4;
    let f = fx_with("value-low", c);
    // an import holding a longer value is declined (the host opens it as text)
    let e = f
        .mgr
        .open(Arc::new(Lite), &key("long.csv"), Some(b"abcdefgh,b\n"))
        .err()
        .expect("declined");
    assert!(
        matches!(&e, SessionError::Declined(d) if d.code == "too_large"),
        "{e:?}"
    );
    let s = f.open("t.csv", "abc,b\n");
    let p = f.person(&s, "p");
    assert!(matches!(
        set(&s, p, "abcde".into()),
        Err(k2_mpc_base::session::EditWithError::Compute(_))
    ));
    assert!(matches!(
        raw(&s, p, "abcde".into()),
        Err(SessionError::AtLimit {
            what: "value bytes",
            limit: 4,
            ..
        })
    ));
    set(&s, p, "abcd".into()).unwrap();
    assert_eq!(text(&s, p), "abcd,b\n");
    // raised
    let mut c = config();
    c.grid_max_value_len = 100 * 1024;
    let f = fx_with("value-high", c);
    let s = f.open("t.csv", "a,b\n");
    let p = f.person(&s, "p");
    set(&s, p, "x".repeat(70 * 1024)).unwrap();
    assert!(raw(&s, p, "y".repeat(100 * 1024 + 1)).is_err());
}

/// C1k (request 8): a pack's tests build an in-memory doc from its decoded content as the
/// session's import does (`session::test_support::import_edits`, feature `test-support`).
#[test]
fn import_edits_builds_the_doc_an_import_makes() {
    let content = Lite.decode(b"a,b\nc,d\n").unwrap();
    let mut d = Doc::new(LITE).unwrap();
    d.edit_as(
        7,
        &k2_mpc_base::session::test_support::import_edits(&content.roots),
    )
    .unwrap();
    let t = Table::from_keyed(&d.keyed_state().unwrap()).unwrap();
    assert_eq!(keys_of(&t.live_rows()), "r1 r2");
    let f = fx("import-edits");
    let s = f.open("t.csv", "a,b\nc,d\n");
    let p = f.person(&s, "p");
    assert_eq!(
        View::of_content(&d.export_neutral().unwrap()).content(),
        s.read(p).unwrap().content,
        "the same content as the session's import"
    );
}

fn keys_of(v: &[Key]) -> String {
    v.iter().map(Key::as_str).collect::<Vec<_>>().join(" ")
}

/// `write --base live` (`Session::write_live_grid`) refused after its reconcile: the column remap
/// it would have made stays with it. Nothing landed, so the next change, by anyone, is not a remap.
mod refused_live_write {
    use super::*;
    use k2_mpc_base::session::{default_reconcile, ClaimPolicy, ClaimSpan, Reconciled};

    /// Maps onto the doc `c,d` by content, so not exactly: two columns swap and one is new.
    const REMAP: &str = "d,c,N\n";
    /// [`REMAP`] with a value over the limit of 4 the tests below set.
    const REMAP_TOO_LONG: &str = "d,c,NNNNN\n";

    fn four_byte_values() -> Fx {
        let mut c = config();
        c.grid_max_value_len = 4;
        fx_with("refused-live", c)
    }

    /// A keyed write as of `old`, on a cell nobody has written since.
    fn stale_write(s: &Session, bot: ConnId, old: u64) -> Result<u64, SessionError> {
        op(s, bot, &EditOptions::new().base(old), |t| {
            t.set_cell(&k("r1"), &k("c1"), "BOT")
        })
    }

    fn unrelated_edit(s: &Session, p: ConnId) {
        person_set(s, p, "r1", "c2", "P");
    }

    fn refused_by_the_limit(s: &Session, bot: ConnId, old: u64) {
        let e = s
            .write_live_grid(
                bot,
                old,
                REMAP_TOO_LONG.as_bytes(),
                &GridWriteOptions::default(),
            )
            .expect_err("the write is refused");
        assert!(
            matches!(
                e,
                SessionError::AtLimit {
                    what: "value bytes",
                    ..
                }
            ),
            "refused at the value limit: {e:?}"
        );
        assert_eq!(s.read(bot).unwrap().version, old, "nothing applied");
    }

    /// With no refused write, the stale write lands.
    #[test]
    fn with_no_refused_write_a_stale_keyed_write_lands() {
        let f = fx("live-control-stale");
        let s = f.open("t.csv", "c,d\n");
        let p = f.person(&s, "p");
        let bot = f.agent(&s, "bot");
        let old = s.read(bot).unwrap().version;
        unrelated_edit(&s, p);
        let r = stale_write(&s, bot, old);
        assert!(r.is_ok(), "{r:?}");
    }

    /// The body is a remap when it lands, so the tests below refuse a write that would be one.
    #[test]
    fn the_body_remaps_the_columns_when_it_lands() {
        let f = fx("live-control-remap");
        let s = f.open("t.csv", "c,d\n");
        let bot = f.agent(&s, "bot");
        let old = s.read(bot).unwrap().version;
        let r = s
            .write_live_grid(bot, old, REMAP.as_bytes(), &GridWriteOptions::default())
            .unwrap();
        assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
        let r = stale_write(&s, bot, old);
        assert!(
            matches!(
                &r,
                Err(SessionError::FacetConflict(c))
                    if c.iter().any(|c| c.rewritten_row && c.root == table::COLS)
            ),
            "{r:?}"
        );
    }

    #[test]
    fn a_write_refused_at_the_limit_does_not_mark_the_next_change() {
        let f = four_byte_values();
        let s = f.open("t.csv", "c,d\n");
        let p = f.person(&s, "p");
        let bot = f.agent(&s, "bot");
        let old = s.read(bot).unwrap().version;
        refused_by_the_limit(&s, bot, old);
        unrelated_edit(&s, p);
        let r = stale_write(&s, bot, old);
        assert!(
            r.is_ok(),
            "the next change was marked a column remap: {r:?}"
        );
    }

    #[test]
    fn a_write_refused_by_a_claim_does_not_mark_the_next_change() {
        let f = fx("live-claim");
        let s = f.open("t.csv", "c,d\n");
        let p = f.person(&s, "p");
        let bot = f.agent(&s, "bot");
        let other = f.agent(&s, "other");
        s.claim_span(
            other,
            ClaimSpan::Rows {
                first: k("r1"),
                last: k("r1"),
            },
            None,
            None,
            false,
        )
        .unwrap();
        let old = s.read(bot).unwrap().version;
        let mut respect = GridWriteOptions::default();
        respect.policy = ClaimPolicy::Respect;
        let e = s
            .write_live_grid(bot, old, REMAP.as_bytes(), &respect)
            .expect_err("the write is refused");
        assert!(matches!(e, SessionError::Claimed { .. }), "{e:?}");
        assert_eq!(s.read(bot).unwrap().version, old, "nothing applied");
        unrelated_edit(&s, p);
        let r = stale_write(&s, bot, old);
        assert!(
            r.is_ok(),
            "the next change was marked a column remap: {r:?}"
        );
    }

    /// A table pack with a `reconcile` of its own that leaves `cols_exact` at its default, the
    /// strict reading ([`Reconciled::cols_exact`]): every write through it is a remap if it lands.
    struct NeverExact;
    impl Pack for NeverExact {
        fn schema(&self) -> Schema {
            Lite.schema()
        }
        fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
            Lite.decode(b)
        }
        fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
            Lite.encode(c, codec)
        }
        fn reconcile(&self, synced: &View, b: &[u8], _: &[u8]) -> Result<Reconciled, Declined> {
            let r = default_reconcile(self, synced, b)?;
            Ok(Reconciled {
                edits: r.edits,
                codec: r.codec,
                ..Reconciled::default()
            })
        }
    }

    /// The `edits.is_empty()` return: the call succeeds and commits nothing, so a flag set before
    /// it reaches the next change, however `commit` treats its flags.
    #[test]
    fn a_write_that_changes_nothing_does_not_mark_the_next_change() {
        let f = fx("live-noop");
        let s = f
            .mgr
            .open(Arc::new(NeverExact), &key("t.csv"), Some(b"c,d\n"))
            .unwrap();
        let p = f.person(&s, "p");
        let bot = f.agent(&s, "bot");
        let old = s.read(bot).unwrap().version;
        let r = s
            .write_live_grid(bot, old, b"c,d\n", &GridWriteOptions::default())
            .unwrap();
        assert!(
            matches!(&r.outcome, GridWrite::Applied(res) if res.version == old),
            "the write changes nothing: {r:?}"
        );
        unrelated_edit(&s, p);
        let r = stale_write(&s, bot, old);
        assert!(
            r.is_ok(),
            "the next change was marked a column remap: {r:?}"
        );
    }

    /// `p`'s edit, then `q`'s write in the same row: undo reads a remap as `p`'s own change
    /// (`rows_rewritten_since`, spec S13) and refuses it.
    fn undo_of_the_next_change(f: &Fx, refuse_first: bool) {
        let s = f.open("t.csv", "c,d\n");
        let p = f.person(&s, "p");
        let q = f.person(&s, "q");
        let bot = f.agent(&s, "bot");
        if refuse_first {
            let old = s.read(bot).unwrap().version;
            refused_by_the_limit(&s, bot, old);
        }
        unrelated_edit(&s, p);
        person_set(&s, q, "r1", "c1", "Q");
        let u = s.undo(p, 1);
        assert!(matches!(&u, Ok(UndoOutcome::Applied { .. })), "{u:?}");
        assert_eq!(text(&s, p), "Q,d\n", "{u:?}");
    }

    #[test]
    fn with_no_refused_write_the_next_change_can_be_undone() {
        undo_of_the_next_change(&four_byte_values(), false);
    }

    #[test]
    fn a_refused_write_does_not_block_undo_of_the_next_change() {
        undo_of_the_next_change(&four_byte_values(), true);
    }
}
