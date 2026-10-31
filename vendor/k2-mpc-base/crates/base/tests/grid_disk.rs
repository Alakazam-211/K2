//! Disk sync for grid docs after a change that moves no byte (the grid twin of
//! `tests/disk.rs`'s `an_outside_edit_after_any_change_that_moved_no_byte_is_kept`, v0.9.4).
//! A tiny table pack (`csv-lite`: one record per line, fields split by `,`, no quoting) on
//! real temp folders, with the kit's default reconcile (`table::reconcile`).
//!
//! - **The property:** a random grid, a random change that leaves the bytes as they were (the
//!   header switched on, or on and off; a cell set to another value and back, or undone; a
//!   row deleted and undone, or restored; a row moved and back; a revert; a version
//!   restore), then nothing, a tick, a flush or a restart, then a random outside edit of the
//!   file. The doc and the file must both end as the outside edit, and the audits (the sync
//!   check and the grid audit) find nothing. `GRID_NO_BYTE_CASES` sets the count (default
//!   256; 10^4 on a node, sharded).
//! - **The planted violation:** a disk replica holding the doc's rendering under other keys
//!   (a row re-keyed; the header off) is `SyncDrift::Elements` while the doc is clean, and the
//!   grid audit reports it too; the next write-back that finds nothing to write heals it.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::doc::{Doc, Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    AuditEvent, AuditViolation, Config, ConnId, Declined, DiskCheck, DiskState, EditOptions, Host,
    Pack, PackContent, PackError, Participant, ParticipantKind, Rights, Session, SessionError,
    SessionManager, Stay, SyncDrift, UndoOutcome, View,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::{self, Field, Record, Shape, Table, TableError};
use proptest::prelude::*;

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
        // (no value limit of its own: the session holds every value to its knob)
        let (_, edits) =
            Table::import_with_max_value(&recs, false, Shape::detect(&recs), usize::MAX).map_err(
                |e| Declined {
                    code: "malformed",
                    message: e.to_string(),
                },
            )?;
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
    _dir: TmpDir,
    ws: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    mgr: SessionManager,
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, |_| {})
}

fn fx_with(tag: &str, set: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-grid-disk-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(p.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    set(&mut c);
    let mgr = SessionManager::new(store.clone(), host.clone(), c);
    Fx {
        _dir: TmpDir(p),
        ws,
        host,
        store,
        mgr,
    }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

impl Fx {
    fn path(&self) -> PathBuf {
        self.ws.join("t.csv")
    }
    fn write(&self, bytes: &str) {
        fs::write(self.path(), bytes).unwrap();
    }
    fn bytes(&self) -> String {
        fs::read_to_string(self.path()).unwrap()
    }
    fn open(&self) -> Arc<Session> {
        self.mgr
            .open_file(Arc::new(Lite), &key("t.csv"), &self.path(), &self.ws)
            .unwrap()
    }
    fn person(&self, s: &Session) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert("p".into(), Rights::full());
        s.join(
            Participant::new("p", "P", "#123456", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn tick(&self) -> k2_mpc_base::session::TickReport {
        let r = self.mgr.tick().unwrap();
        assert!(r.errors.is_empty(), "tick errors: {:?}", r.errors);
        r
    }
    /// A crash: a new manager on the same store, the old one dropped without a write-back.
    fn restart(&mut self) {
        self.mgr = SessionManager::new(
            self.store.clone(),
            self.host.clone(),
            self.mgr.config().clone(),
        );
    }
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn text(s: &Session, c: ConnId) -> String {
    String::from_utf8(s.read(c).unwrap().bytes).unwrap()
}

fn disk(s: &Session) -> DiskState {
    s.status().unwrap().disk.unwrap()
}

/// Run a table op on the live table as `c`; the change's version.
fn op(
    s: &Session,
    c: ConnId,
    f: impl FnOnce(&mut Table) -> Result<Vec<Edit>, TableError>,
) -> Result<u64, SessionError> {
    match s.edit_ops_with(c, &EditOptions::new(), |v| {
        let mut t = v.table().map_err(|e| e.to_string())?;
        f(&mut t).map_err(|e| e.to_string())
    }) {
        Ok(r) => Ok(r.map_or(0, |r| r.version)),
        Err(k2_mpc_base::session::EditWithError::Session(e)) => Err(e),
        Err(k2_mpc_base::session::EditWithError::Compute(e)) => panic!("table op: {e}"),
    }
}

fn live(s: &Session, c: ConnId) -> Table {
    View::of_content(&s.read(c).unwrap().content)
        .table()
        .unwrap()
}

/// Both audits find nothing: the sync check, and the grid audit (its (d) is the same check).
fn audits_clean(s: &Session, why: &str) {
    assert_eq!(s.audit_disk_sync().unwrap(), vec![], "{why}: K-AUD-1 sync");
    let rep = s.audit().unwrap();
    assert!(rep.is_clean(), "{why}: grid audit {:?}", rep.violations);
}

// ---------- the planted violation ----------

#[test]
fn the_sync_audit_reports_a_grid_replica_under_other_keys() {
    let f = fx("plant");
    f.write("a,b\nc,d\n");
    let s = f.open();
    let p = f.person(&s);
    audits_clean(&s, "as imported");
    // a row re-keyed on the replica: the same rendering under another key
    s.plant_disk_replica_edits_for_tests(&[Edit::List {
        root: table::ROWS.into(),
        ops: vec![
            ListOp::Delete { key: k("r2") },
            ListOp::Insert {
                key: k("r9"),
                at: Anchor::After(k("r1")),
                value: Value::Null,
            },
        ],
    }])
    .unwrap();
    let rows = || SyncDrift::Elements {
        root: table::ROWS.into(),
    };
    assert_eq!(s.audit_disk_sync().unwrap(), vec![rows()]);
    let rep = s.audit().unwrap();
    assert_eq!(rep.violations, vec![AuditViolation::DiskReplica(rows())]);
    // a write-back with nothing to write brings it up to the doc
    assert!(!s.flush(p).unwrap().written);
    audits_clean(&s, "re-synced");
    // the header switched on the replica only (the stale base of the binding's repro)
    s.plant_disk_replica_edits_for_tests(&[Edit::Map {
        root: table::META.into(),
        ops: vec![MapOp::Set {
            key: "header".into(),
            value: Value::str("on"),
        }],
    }])
    .unwrap();
    assert_eq!(
        s.audit_disk_sync().unwrap(),
        vec![SyncDrift::Elements {
            root: table::META.into()
        }]
    );
    // only a clean doc is audited: unwritten changes are the replica's to merge
    op(&s, p, |t| t.set_cell(&k("r1"), &k("c1"), "A")).unwrap();
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
    assert!(s.flush(p).unwrap().written);
    audits_clean(&s, "written");
}

/// The binding's repro (k2-mpc-csv `an_outside_edit_after_a_header_switch_is_kept`) on the
/// kit's own test pack: `header on` moves no byte; a flush finds nothing to write; the header
/// line renamed outside is the doc's, and the file keeps it.
#[test]
fn an_outside_rename_after_a_header_switch_is_kept() {
    for restart in [false, true] {
        let mut f = fx("header-stale");
        f.write("name,city\nAna,Oslo\n");
        let mut s = f.open();
        let mut p = f.person(&s);
        op(&s, p, |t| t.set_header(true)).unwrap();
        assert!(!s.flush(p).unwrap().written, "no bytes changed");
        audits_clean(&s, "after the switch");
        if restart {
            drop(s);
            f.restart();
            s = f.open();
            p = f.person(&s);
        }
        f.advance(1_000);
        f.write("name,town\nAna,Oslo\n");
        assert!(matches!(
            s.file_changed().unwrap(),
            DiskCheck::Ingested { .. }
        ));
        assert_eq!(
            text(&s, p),
            "name,town\nAna,Oslo\n",
            "the rename is the doc's"
        );
        assert_eq!(live(&s, p).name(&k("c2")), Some("town"));
        s.flush(p).unwrap();
        assert_eq!(f.bytes(), "name,town\nAna,Oslo\n", "and the file keeps it");
        audits_clean(&s, "at the end");
    }
}

// ---------- the value limit on outside saves (C1k, request 7) ----------

/// With `Config::grid_max_value_len` raised, an outside save carrying a value over 64 KiB is
/// accepted (the aligner reads the file under the host's limit); at the default it is
/// refused (the file is declined, never written over).
#[test]
fn an_outside_save_follows_the_value_limit() {
    let long = "x".repeat(70 * 1024);
    let body = format!("a,{long}\nc,d\n");
    // raised
    let f = fx_with("value-raised", |c| c.grid_max_value_len = 100 * 1024);
    f.write("a,b\nc,d\n");
    let s = f.open();
    let p = f.person(&s);
    f.advance(1_000);
    f.write(&body);
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(text(&s, p), body);
    assert_eq!(live(&s, p).cell(&k("r1"), &k("c2")), Some(long.as_str()));
    audits_clean(&s, "raised");
    // the default
    let f = fx("value-default");
    f.write("a,b\nc,d\n");
    let s = f.open();
    let p = f.person(&s);
    f.advance(1_000);
    f.write(&body);
    let c = s.file_changed().unwrap();
    assert!(matches!(c, DiskCheck::Declined(_)), "{c:?}");
    assert_eq!(text(&s, p), "a,b\nc,d\n", "the doc keeps its content");
    assert_eq!(f.bytes(), body, "the file isn't written over");
}

// ---------- a re-save that changes only the codec settings (C1k, binding rule (b)) ----------

/// `csv-lite` with a dialect: a BOM, CRLF and `;` as the delimiter are the codec settings
/// (`bom,crlf,;` as bytes), each read from the file and written back.
struct Dialect;

fn dialect_of(b: &[u8]) -> (bool, bool, char, String) {
    let s = String::from_utf8_lossy(b);
    let bom = s.starts_with('\u{feff}');
    let s = s.trim_start_matches('\u{feff}');
    let crlf = s.contains("\r\n");
    let d = if s.lines().next().unwrap_or("").contains(';') {
        ';'
    } else {
        ','
    };
    (bom, crlf, d, s.replace("\r\n", "\n").replace(d, ","))
}

impl Pack for Dialect {
    fn schema(&self) -> Schema {
        LITE
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (bom, crlf, d, plain) = dialect_of(b);
        let mut c = Lite.decode(plain.as_bytes())?;
        c.codec = format!("{},{},{d}", u8::from(bom), u8::from(crlf)).into_bytes();
        Ok(c)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let s = String::from_utf8_lossy(codec).into_owned();
        let f: Vec<&str> = s.splitn(3, ',').collect();
        let plain = String::from_utf8(Lite.encode(c, &[])?).unwrap();
        let mut out = plain.replace(',', f[2]);
        if f[1] == "1" {
            out = out.replace('\n', "\r\n");
        }
        if f[0] == "1" {
            out.insert(0, '\u{feff}');
        }
        Ok(out.into_bytes())
    }
    fn describe_codec_change(&self, old: &[u8], new: &[u8]) -> String {
        format!(
            "{} -> {}",
            String::from_utf8_lossy(old),
            String::from_utf8_lossy(new)
        )
    }
}

#[test]
fn a_re_save_that_changes_only_the_dialect_is_an_attributed_outside_change() {
    use k2_mpc_base::session::{AuditEvent as Ev, LogItem, LogQuery, EXTERNAL_PID};
    use k2_mpc_base::store::VersionReason;
    let seed = "a,b\nc,d\n";
    let cases: [(&str, &str, &str); 3] = [
        ("bom", "\u{feff}a,b\nc,d\n", "0,0,, -> 1,0,,"),
        ("crlf", "a,b\r\nc,d\r\n", "0,0,, -> 0,1,,"),
        ("delimiter", "a;b\nc;d\n", "0,0,, -> 0,0,;"),
    ];
    for (tag, resave, want) in cases {
        let f = fx(&format!("settings-{tag}"));
        f.write(seed);
        let s = f
            .mgr
            .open_file(Arc::new(Dialect), &key("t.csv"), &f.path(), &f.ws)
            .unwrap();
        let p = f.person(&s);
        let rx = s.subscribe(p).unwrap();
        let settings_rx = s.subscribe_settings(p).unwrap();
        let before = s.status().unwrap().version;
        let versions = s.versions(p).unwrap().len();
        f.advance(1_000);
        f.write(resave);
        assert_eq!(s.file_changed().unwrap(), DiskCheck::Unchanged, "{tag}");
        // the content is unchanged; the doc's version doesn't move
        assert_eq!(live(&s, p).render(), records(seed), "{tag}");
        assert_eq!(s.status().unwrap().version, before, "{tag}");
        assert_eq!(text(&s, p), resave, "{tag}: the doc writes the new dialect");
        // a version by the external participant holds the new settings
        let vs = s.versions(p).unwrap();
        assert_eq!(vs.len(), versions + 1, "{tag}");
        let v = vs.iter().max_by_key(|v| v.id).unwrap();
        assert_eq!(v.reason, VersionReason::External, "{tag}");
        assert_eq!(v.authors, [EXTERNAL_PID.to_owned()], "{tag}");
        // log and show: an outside settings change, described by the pack
        let log = s.log(p, &LogQuery::default()).unwrap();
        let found = log.iter().find_map(|i| match i {
            LogItem::Settings {
                version,
                description,
            } => Some((version.id, description.clone())),
            _ => None,
        });
        assert_eq!(found, Some((v.id, want.to_owned())), "{tag}: {log:?}");
        assert_eq!(s.settings_change(p, v.id).unwrap().as_deref(), Some(want));
        // the events: an audit event naming the version, and a settings event (its own
        // subscription: the content and the version didn't change, so no change event, and
        // nothing a replica following versions would take for one it has seen)
        assert!(
            f.events().iter().any(|e| matches!(e,
                Ev::OutsideSettings { version, description, .. }
                    if *version == v.id && description == want)),
            "{tag}"
        );
        assert!(rx.try_recv().is_err(), "{tag}: no change event");
        let ev = settings_rx.try_recv().expect("a settings event");
        assert_eq!(ev.description, want, "{tag}");
        assert_eq!(ev.settings_version, v.id, "{tag}");
        assert_eq!(ev.pid, EXTERNAL_PID, "{tag}");
        assert_eq!(ev.version, before, "{tag}");
        assert_eq!(
            Dialect
                .encode(&s.read(p).unwrap().content, &ev.codec)
                .unwrap(),
            resave.as_bytes(),
            "{tag}: the event carries the new settings"
        );
        // the file is the re-save; a write-back finds nothing to write
        assert!(!s.flush(p).unwrap().written, "{tag}");
        assert_eq!(f.bytes(), resave, "{tag}");
        audits_clean(&s, tag);
        // a re-save with no dialect change produces nothing
        let n = s.versions(p).unwrap().len();
        let events = f.events().len();
        f.advance(1_000);
        f.write(resave);
        let c = s.file_changed().unwrap();
        assert!(matches!(c, DiskCheck::Unchanged), "{tag}: {c:?}");
        assert_eq!(s.versions(p).unwrap().len(), n, "{tag}: no version");
        assert!(
            !f.events()[events..]
                .iter()
                .any(|e| matches!(e, Ev::OutsideSettings { .. })),
            "{tag}"
        );
        assert!(rx.try_recv().is_err(), "{tag}: no event");
        assert!(settings_rx.try_recv().is_err(), "{tag}: no settings event");
        // an older version (before the re-save) is no settings change
        assert_eq!(s.settings_change(p, vs[0].id).unwrap(), None);
    }
}

// ---------- the property ----------// ---------- the property ----------

/// `GRID_NO_BYTE_CASES` (default 256; 10^4 on a node, in shards: each run draws its own
/// random seed).
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

fn cases() -> u32 {
    std::env::var("GRID_NO_BYTE_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256)
}

/// A change that moves no byte, by every route a grid doc offers.
#[derive(Debug, Clone, Copy)]
enum NoByte {
    HeaderOn,
    HeaderOnOff,
    CellBack,
    CellUndo,
    RowDeleteUndo,
    RowDeleteRestore,
    RowMoveBack,
    Revert,
    Restore,
}

/// What happens between that change and the outside edit.
#[derive(Debug, Clone, Copy)]
enum Between {
    Nothing,
    Tick,
    Flush,
    /// A crash: the manager dropped without a write-back, and the doc reopened.
    Restart,
    FlushRestart,
}

/// One outside edit of the file's lines (the header line included, when it is one).
#[derive(Debug, Clone, Copy)]
enum Outside {
    /// A field of a line set to a new value.
    Field(u16, u16),
    /// A new line (as wide as the table) before a line.
    Insert(u16),
    Delete(u16),
    /// A line moved to another place.
    Move(u16, u16),
}

fn grid() -> impl Strategy<Value = Vec<Vec<&'static str>>> {
    (1usize..4)
        .prop_flat_map(|w| {
            prop::collection::vec(
                prop_oneof![
                    6 => prop::collection::vec(prop::sample::select(vec!["a", "b", "x", "Ana"]), w),
                    1 => Just(Vec::new()),
                ],
                2..7,
            )
        })
        .prop_filter("a line that isn't blank", |g| {
            g.iter().any(|r| !r.is_empty())
        })
}

fn no_byte() -> impl Strategy<Value = NoByte> {
    prop::sample::select(vec![
        NoByte::HeaderOn,
        NoByte::HeaderOnOff,
        NoByte::CellBack,
        NoByte::CellUndo,
        NoByte::RowDeleteUndo,
        NoByte::RowDeleteRestore,
        NoByte::RowMoveBack,
        NoByte::Revert,
        NoByte::Restore,
    ])
}

fn between() -> impl Strategy<Value = Between> {
    prop::sample::select(vec![
        Between::Nothing,
        Between::Tick,
        Between::Flush,
        Between::Restart,
        Between::FlushRestart,
    ])
}

fn outside() -> impl Strategy<Value = Outside> {
    prop_oneof![
        3 => (any::<u16>(), any::<u16>()).prop_map(|(a, b)| Outside::Field(a, b)),
        1 => any::<u16>().prop_map(Outside::Insert),
        1 => any::<u16>().prop_map(Outside::Delete),
        1 => (any::<u16>(), any::<u16>()).prop_map(|(a, b)| Outside::Move(a, b)),
    ]
}

fn render(lines: &[Vec<String>]) -> String {
    lines.iter().map(|l| format!("{}\n", l.join(","))).collect()
}

/// The outside edit on the file's lines; `None` when it would leave the file in a form this
/// test pack can't hold under the doc's settings (fewer than two lines, or a blank line edited
/// into a short one).
fn outside_edit(file: &str, width: usize, e: Outside) -> Option<String> {
    let mut lines: Vec<Vec<String>> = file
        .lines()
        .map(|l| {
            if l.is_empty() {
                Vec::new()
            } else {
                l.split(',').map(str::to_owned).collect()
            }
        })
        .collect();
    let n = lines.len();
    let at = |x: u16, m: usize| x as usize % m;
    match e {
        Outside::Field(l, j) => {
            let l = at(l, n);
            if lines[l].is_empty() {
                // a blank line gets a full row
                lines[l] = (0..width).map(|j| format!("N{j}")).collect();
            } else {
                let j = at(j, lines[l].len());
                lines[l][j] = format!("N{}", lines[l][j]);
            }
        }
        Outside::Insert(l) => {
            let l = at(l, n + 1);
            lines.insert(l, (0..width).map(|j| format!("I{j}")).collect());
        }
        Outside::Delete(l) => {
            if n <= 2 {
                return None;
            }
            lines.remove(at(l, n));
        }
        Outside::Move(a, b) => {
            let line = lines.remove(at(a, n));
            lines.insert(at(b, n), line);
        }
    }
    let out = render(&lines);
    (out != file).then_some(out)
}

fn no_byte_case(grid: &[Vec<&str>], change: NoByte, pick: u16, between: Between, out: Outside) {
    let why = format!("{change:?} then {between:?}, outside {out:?}");
    let width = grid.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let seed = render(
        &grid
            .iter()
            .map(|r| r.iter().map(|v| (*v).to_owned()).collect())
            .collect::<Vec<_>>(),
    );
    let mut f = fx("nobyte-prop");
    f.write(&seed);
    let mut s = f.open();
    let p = f.person(&s);
    assert_eq!(text(&s, p), seed, "{why}: imported as is");
    let t = live(&s, p);
    let rows = t.live_rows();
    let row = rows[pick as usize % rows.len()].clone();
    let cols = t.live_cols();
    let col = cols[pick as usize % cols.len()].clone();
    match change {
        NoByte::HeaderOn | NoByte::HeaderOnOff => {
            op(&s, p, |t| t.set_header(true)).unwrap();
            if matches!(change, NoByte::HeaderOnOff) {
                op(&s, p, |t| t.set_header(false)).unwrap();
            }
        }
        NoByte::CellBack => {
            let was = t.cell(&row, &col).map(str::to_owned);
            op(&s, p, |t| t.set_cell(&row, &col, "zz")).unwrap();
            match was {
                Some(v) => op(&s, p, |t| t.set_cell(&row, &col, &v)).unwrap(),
                // an absent cell set and cleared back to absent: an undo
                None => {
                    assert!(matches!(s.undo(p, 1).unwrap(), UndoOutcome::Applied { .. }));
                    0
                }
            };
        }
        NoByte::CellUndo => {
            op(&s, p, |t| t.set_cell(&row, &col, "zz")).unwrap();
            let u = s.undo(p, 1).unwrap();
            assert!(matches!(u, UndoOutcome::Applied { .. }), "{why}: {u:?}");
        }
        NoByte::RowDeleteUndo => {
            op(&s, p, |t| t.delete_row(&row)).unwrap();
            let u = s.undo(p, 1).unwrap();
            assert!(matches!(u, UndoOutcome::Applied { .. }), "{why}: {u:?}");
        }
        NoByte::RowDeleteRestore => {
            op(&s, p, |t| t.delete_row(&row)).unwrap();
            op(&s, p, |t| t.restore_row(&row)).unwrap();
        }
        NoByte::RowMoveBack => {
            let i = rows.iter().position(|r| *r == row).unwrap();
            let back = match i {
                0 => Anchor::Start,
                i => Anchor::After(rows[i - 1].clone()),
            };
            let to = match rows.last() {
                Some(last) if *last != row => Anchor::After(last.clone()),
                _ => Anchor::Start,
            };
            op(&s, p, |t| t.move_row(&row, to)).unwrap();
            op(&s, p, |t| t.move_row(&row, back)).unwrap();
        }
        NoByte::Revert => {
            let v = op(&s, p, |t| t.set_cell(&row, &col, "zz")).unwrap();
            let r = s.revert(p, v, false).unwrap();
            assert!(matches!(r, UndoOutcome::Applied { .. }), "{why}: {r:?}");
        }
        NoByte::Restore => {
            let v = s.name_version(p, "before", None).unwrap();
            op(&s, p, |t| t.set_cell(&row, &col, "zz")).unwrap();
            s.restore(p, v).unwrap();
        }
    }
    assert_eq!(text(&s, p), seed, "{why}: the change moved no byte");
    if matches!(between, Between::Flush | Between::FlushRestart) {
        assert!(!s.flush(p).unwrap().written, "{why}: nothing to write");
        audits_clean(&s, &format!("{why}: after the flush"));
    }
    if matches!(between, Between::Tick) {
        f.advance(10_000);
        assert!(f.tick().written.is_empty(), "{why}: nothing to write");
        audits_clean(&s, &format!("{why}: after the tick"));
    }
    let mut p = p;
    if matches!(between, Between::Restart | Between::FlushRestart) {
        drop(s);
        f.restart();
        s = f.open();
        p = f.person(&s);
        assert_eq!(disk(&s), DiskState::Clean, "{why}: reopened clean");
        audits_clean(&s, &format!("{why}: reopened"));
    }
    assert_eq!(f.bytes(), seed, "{why}: the file untouched");
    let Some(edited) = outside_edit(&seed, width, out) else {
        return;
    };
    f.advance(1_000);
    f.write(&edited);
    let check = s.file_changed().unwrap();
    assert!(
        matches!(check, DiskCheck::Ingested { .. }),
        "{why}: {check:?}"
    );
    assert_eq!(text(&s, p), edited, "{why}: the doc is the outside edit");
    f.advance(10_000);
    f.tick();
    s.flush(p).unwrap();
    assert_eq!(f.bytes(), edited, "{why}: the file is the outside edit");
    assert_eq!(disk(&s), DiskState::Clean, "{why}");
    audits_clean(&s, &format!("{why}: at the end"));
}

proptest! {
    #![proptest_config(seeded(cases(), "an_outside_edit_after_any_grid_change_that_moved_no_byte_is_kept"))]
    #[test]
    fn an_outside_edit_after_any_grid_change_that_moved_no_byte_is_kept(
        grid in grid(),
        change in no_byte(),
        pick in any::<u16>(),
        between in between(),
        out in outside(),
    ) {
        no_byte_case(&grid, change, pick, between, out);
    }
}

fn mismatches(f: &Fx) -> usize {
    f.events()
        .iter()
        .filter(|e| matches!(e, AuditEvent::DiskReplicaMismatch { .. }))
        .count()
}

/// An outside save while the doc is dirty (an edit not yet written back), with and without a
/// header switch before it, raises no `DiskReplicaMismatch`: the replica held the file's state
/// as its own ops, so it reloads from the doc at the next write-back instead of being compared
/// (no kit bug to report), and then equals the doc. Raised at every such save before.
#[test]
fn a_dirty_outside_save_raises_no_replica_mismatch() {
    for header in [false, true] {
        let f = fx(&format!("dirty-save-{header}"));
        f.write("name,city\nAna,Oslo\nBen,Rome\n");
        let s = f.open();
        let p = f.person(&s);
        if header {
            op(&s, p, |t| t.set_header(true)).unwrap();
        }
        op(&s, p, |t| t.set_cell(&k("r3"), &k("c2"), "ROME")).unwrap();
        assert!(matches!(disk(&s), DiskState::Dirty { .. }));
        f.advance(100);
        f.write("name,town\nAna,Oslo\nBen,Rome\n");
        assert!(matches!(
            s.file_changed().unwrap(),
            DiskCheck::Ingested { .. }
        ));
        f.advance(10_000);
        f.tick();
        s.flush(p).unwrap();
        assert_eq!(
            text(&s, p),
            "name,town\nAna,Oslo\nBen,ROME\n",
            "header={header}"
        );
        assert_eq!(
            f.bytes(),
            "name,town\nAna,Oslo\nBen,ROME\n",
            "header={header}"
        );
        assert_eq!(mismatches(&f), 0, "header={header}: {:?}", f.events());
        audits_clean(&s, &format!("header={header}: the replica is the doc"));
        // and the next cycle compares again, cleanly
        op(&s, p, |t| t.set_cell(&k("r2"), &k("c2"), "OSLO")).unwrap();
        f.advance(10_000);
        f.tick();
        s.flush(p).unwrap();
        assert_eq!(mismatches(&f), 0, "header={header}: {:?}", f.events());
        audits_clean(&s, &format!("header={header}: after the next write-back"));
    }
}

/// A real replica mismatch is still reported: an edit planted on the replica behind the doc's
/// back (`plant_disk_replica_edits_for_tests`), then an ordinary edit and its write-back. And the
/// same plant in the cycle of a dirty outside save: the save's replica ops make that cycle
/// reload from the doc without a compare, so the planted edit is corrected there, not reported.
#[test]
fn a_planted_replica_mismatch_is_still_reported() {
    let plant = |s: &Session| {
        s.plant_disk_replica_edits_for_tests(&[Edit::Map {
            root: table::CELLS.into(),
            ops: vec![MapOp::Set {
                key: table::cell_key(&k("r1"), &k("c1")),
                value: Value::str("PLANTED"),
            }],
        }])
        .unwrap();
    };
    {
        let f = fx("plant-reported");
        f.write("a,b\nc,d\n");
        let s = f.open();
        let p = f.person(&s);
        plant(&s);
        op(&s, p, |t| t.set_cell(&k("r2"), &k("c2"), "D")).unwrap();
        f.advance(10_000);
        f.tick();
        s.flush(p).unwrap();
        println!("plant then an edit: events {:?}", f.events());
        assert_eq!(mismatches(&f), 1, "reported: {:?}", f.events());
        assert_eq!(f.bytes(), "a,b\nc,D\n", "the file is the doc's");
    }
    {
        let f = fx("plant-in-dirty-save");
        f.write("a,b\nc,d\n");
        let s = f.open();
        let p = f.person(&s);
        plant(&s);
        op(&s, p, |t| t.set_cell(&k("r2"), &k("c2"), "D")).unwrap();
        f.advance(100);
        f.write("a,B\nc,d\n");
        let c = s.file_changed();
        f.advance(10_000);
        f.tick();
        s.flush(p).unwrap();
        println!(
            "plant in a dirty save's cycle: {c:?}; text {:?}; file {:?}; events {:?}; sync {:?}",
            text(&s, p),
            f.bytes(),
            f.events(),
            s.audit_disk_sync()
        );
        assert_eq!(text(&s, p), "a,B\nc,D\n");
        assert_eq!(f.bytes(), "a,B\nc,D\n");
        assert_eq!(
            s.audit_disk_sync().unwrap(),
            vec![],
            "corrected by the reload"
        );
    }
}

/// The C1k case (once open, ignored): the grid form of `tests/disk.rs`'s
/// `an_outside_edit_after_a_no_byte_change_and_an_unwritten_edit_is_kept`. `header on` moves
/// no byte, and an edit not yet written back leaves the doc dirty; the header line's outside
/// rename must stand. Fixed by the keyed three-way merge of outside saves (c080489) and its
/// re-identification step (828faf8: `ingest_structured` gives the replica the live keys first).
/// `set_header(true)` deletes the first row and keeps its values as the column names, so that
/// row's hidden cells stay stale (`name,city`); `set_header(false)` mints a new row from the
/// names, so they never come back. (Such a save raised a false `DiskReplicaMismatch` until
/// efe34be: `a_dirty_outside_save_raises_no_replica_mismatch`.)
#[test]
fn an_outside_rename_after_a_header_switch_and_an_unwritten_edit_is_kept() {
    let f = fx("header-dirty");
    f.write("name,city\nAna,Oslo\nBen,Rome\n");
    let s = f.open();
    let p = f.person(&s);
    op(&s, p, |t| t.set_header(true)).unwrap();
    op(&s, p, |t| t.set_cell(&k("r3"), &k("c2"), "ROME")).unwrap();
    assert!(matches!(disk(&s), DiskState::Dirty { .. }));
    f.advance(100);
    f.write("name,town\nAna,Oslo\nBen,Rome\n");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(text(&s, p), "name,town\nAna,Oslo\nBen,ROME\n");
}
