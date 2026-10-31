//! C1f: erasure for structured (grid) docs (A13, K-PURGE-1 amended, CSV seam design §7.4),
//! public API only, with the tiny `csv-lite` table pack (`grid_store.rs`'s).
//!
//! - **`purge::byte_scan`, grid form:** secrets planted in a live cell, a hidden cell (of a
//!   deleted row), an overwritten value, a named version, a column name, and one split
//!   across two cells set in two changes. Before the purge the store file holds every one
//!   (the negative control) and the doc's file the live ones; after it, neither holds any
//!   byte of them (whole or by any 8-byte window). By text (`and_remove` for live and hidden
//!   values), and by a whole-history purge.
//! - **Scopes:** `--cell` and `--row` redact the element's values whole in every version and
//!   record, and copies of them elsewhere; other cells keep theirs; live values stay.
//! - **A revert over a purged value reports `erased`**, and puts nothing back.
//! - **Purge then restore never resurrects** (`purge_then_restore_never_resurrects`): random
//!   grid histories with a secret planted somewhere; after `purge --text --and-remove`, no
//!   version restore, revert, undo or redo brings any of it back, live or in the store.
//!   `GRID_ERASURE_CASES` histories (default 32; 10^4 on a node).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::doc::{Doc, Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, MapOp, Value};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    AuditEvent, Config, ConflictKind, ConnId, Declined, EditOptions, Host, Pack, PackContent,
    PackError, Participant, ParticipantKind, PurgeRequest, PurgeScope, PurgedScope, Rights,
    Session, SessionError, SessionManager, Stay, UndoOutcome, View,
};
use k2_mpc_base::store::{scan_file, DocKey, DocStore, Finding};
use k2_mpc_base::table::{self, Field, Record, Shape, Table, TableError};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

// ---------- the test pack (as grid_store.rs) ----------

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

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    owner: Mutex<String>,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        if *self.owner.lock().unwrap() == who.pid {
            Rights::full().with_owner(true)
        } else {
            Rights::full()
        }
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct Fx {
    dir: PathBuf,
    file: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn config() -> Config {
    let mut c = Config::default();
    c.edit_burst = 1_000_000;
    c.edit_rate_per_s = 1_000_000;
    // write-backs only when asked (flush), so the file is a planted place on demand
    c.write_quiet_ms = 1 << 40;
    c.write_max_delay_ms = 1 << 40;
    c
}

/// A grid doc synced with a file, opened from `body`.
fn fx(tag: &str, body: &str) -> (Fx, Arc<Session>) {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-grid-erase-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let files = dir.join("files");
    std::fs::create_dir_all(&files).unwrap();
    let files = std::fs::canonicalize(files).unwrap();
    let file = files.join("t.csv");
    std::fs::write(&file, body).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        owner: Mutex::new("own".into()),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), config());
    let key = DocKey {
        space: "ws".into(),
        path: "t.csv".into(),
    };
    let s = mgr.open_file(Arc::new(Lite), &key, &file, &files).unwrap();
    (
        Fx {
            dir,
            file,
            host,
            store,
            mgr,
        },
        s,
    )
}

fn join(s: &Session, pid: &str) -> ConnId {
    s.join(
        Participant::new(
            pid,
            pid.to_uppercase(),
            "#123456",
            ParticipantKind::Person,
            "cli",
        ),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn secret() -> String {
    let b: [u8; 24] = rand::random();
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Run a table op on the live table as `c`.
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

/// [`op`], a table refusal (an op the table can't make here) being `Ok(0)`: random histories.
fn try_op(
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
        Err(k2_mpc_base::session::EditWithError::Compute(_)) => Ok(0),
    }
}

/// A body with a header line: [`header_on`] makes it the column names (its row, `r1`, is
/// then deleted: the data rows are `r2`, `r3`, `r4`).
const BODY: &str = "h1,h2\na,b\nc,d\ne,f\n";

fn header_on(s: &Session, c: ConnId) {
    op(s, c, |t| t.set_header(true)).unwrap();
}

/// Set every map value (a cell, hidden ones too, a name...) holding any 8-byte window of `t`
/// to `""`, by key, as `c`: editing a secret out of the live doc.
fn clear_values(s: &Session, c: ConnId, t: &str) {
    let mut edits = Vec::new();
    for (root, n) in s.read(c).unwrap().content {
        if let Neutral::Map(m) = n {
            let ops: Vec<MapOp> = m
                .into_iter()
                .filter(|(_, v)| matches!(v, Value::Str(x) if holds_window(x, t)))
                .map(|(key, _)| MapOp::Set {
                    key,
                    value: Value::str(""),
                })
                .collect();
            if !ops.is_empty() {
                edits.push(Edit::Map { root, ops });
            }
        }
    }
    if !edits.is_empty() {
        s.edit_ops(c, edits, &EditOptions::new()).unwrap();
    }
}

fn set(s: &Session, c: ConnId, r: &str, col: &str, v: &str) -> u64 {
    op(s, c, |t| t.set_cell(&k(r), &k(col), v)).unwrap()
}

fn hits(f: &[Finding]) -> Vec<usize> {
    let mut v: Vec<usize> = f.iter().map(|f| f.needle).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn in_store(store: &DocStore, needles: &[&str]) -> Vec<usize> {
    let b: Vec<&[u8]> = needles.iter().map(|s| s.as_bytes()).collect();
    hits(&store.scan_for(&b).unwrap())
}

fn in_file(p: &Path, needles: &[&str]) -> Vec<usize> {
    let b: Vec<&[u8]> = needles.iter().map(|s| s.as_bytes()).collect();
    hits(&scan_file(p, &b).unwrap())
}

/// Every value of the live doc, hidden ones too, as one string per value.
fn live_values(s: &Session, c: ConnId) -> Vec<String> {
    let mut out = Vec::new();
    for (_, n) in s.read(c).unwrap().content {
        match n {
            Neutral::Map(m) => out.extend(m.into_values().filter_map(|v| match v {
                Value::Str(s) => Some(s),
                _ => None,
            })),
            Neutral::List(l) => out.extend(l.into_iter().filter_map(|(_, v)| match v {
                Value::Str(s) => Some(s),
                _ => None,
            })),
            Neutral::Text(t) => out.push(t),
            _ => {}
        }
    }
    out
}

fn holds_window(hay: &str, secret: &str) -> bool {
    secret
        .as_bytes()
        .windows(8)
        .any(|w| hay.as_bytes().windows(8).any(|h| h == w))
}

/// The six planted places (CSV seam design §7.4). Returns the secrets, in order: a live
/// cell, a hidden cell, an overwritten value, a named version, a column name, and one split
/// across two cells set in two changes.
fn plant_six(f: &Fx, s: &Session, ed: ConnId) -> [String; 6] {
    let six = [secret(), secret(), secret(), secret(), secret(), secret()];
    header_on(s, ed);
    // 1. a live cell, written to the file too
    set(s, ed, "r2", "c1", &six[0]);
    s.flush(ed).unwrap();
    // 2. a hidden cell: its row deleted
    set(s, ed, "r3", "c2", &six[1]);
    op(s, ed, |t| t.delete_row(&k("r3"))).unwrap();
    // 3. an overwritten value
    set(s, ed, "r4", "c1", &six[2]);
    set(s, ed, "r4", "c1", "plain");
    // 4. a named version (then overwritten)
    set(s, ed, "r4", "c2", &six[3]);
    s.name_version(ed, "with a secret", None).unwrap();
    set(s, ed, "r4", "c2", "after");
    // 5. a column name (then renamed)
    op(s, ed, |t| t.rename_col(&k("c2"), &six[4])).unwrap();
    op(s, ed, |t| t.rename_col(&k("c2"), "second")).unwrap();
    // 6. split across two cells, set in two changes, then cleared
    let (h1, h2) = six[5].split_at(24);
    set(s, ed, "r4", "c1", h1);
    set(s, ed, "r2", "c2", h2);
    set(s, ed, "r4", "c1", "plain");
    set(s, ed, "r2", "c2", "x");
    let _ = f;
    six
}

// ---------- purge::byte_scan, grid form ----------

/// The six planted places, purged by text (`and_remove` for the live and the hidden cell):
/// nothing left in the store file or the doc's file.
/// A waiting Overwritten note holds a capped live value in the store: a purge takes it, leaving
/// the fact and the count; no byte survives, in the note's table or the file.
#[test]
fn a_waiting_notes_value_is_purged_with_the_rest() {
    let (f, s) = fx("note", BODY);
    let own = join(&s, "own");
    let ed = join(&s, "ed");
    let live = secret();
    let row = s.read(ed).unwrap();
    let _ = row;
    set(&s, ed, "r1", "c1", &live);
    // an outside save over that row: the note to ed holds the live value
    let text = String::from_utf8(s.read(ed).unwrap().bytes).unwrap();
    let first = text.lines().next().unwrap().to_owned();
    let file_row = first.replacen(&live, "FROMFILE", 1);
    let saved = text.replacen(&first, &file_row, 1);
    f.host.now.fetch_add(500, Ordering::SeqCst);
    std::fs::write(&f.file, &saved).unwrap();
    s.file_changed().unwrap();
    assert_eq!(
        in_store(&f.store, &[&live]),
        vec![0],
        "the control: the store holds it"
    );
    s.purge(own, PurgeRequest::new(PurgeScope::Text(live.clone())))
        .unwrap();
    assert_eq!(in_store(&f.store, &[&live]), Vec::<usize>::new());
    let n = s.take_notes(ed).unwrap();
    let o = n.notes[0].overwrite.as_ref().expect("the note stays");
    assert!(o.withheld, "{o:?}");
    assert!(o.yours.is_empty() && o.file.is_empty());
}

/// The same for a `dropped_col` note: a purge takes its value too.
#[test]
fn a_dropped_columns_note_value_is_purged() {
    let (f, s) = fx("dropcol", BODY);
    let own = join(&s, "own");
    let ed = join(&s, "ed");
    let live = secret();
    set(&s, ed, "r1", "c2", &live);
    op(&s, own, |t| t.delete_col(&k("c2"))).unwrap();
    s.drop_deleted(own).unwrap();
    assert_eq!(
        in_store(&f.store, &[&live]),
        vec![0],
        "the control: the store holds it"
    );
    s.purge(own, PurgeRequest::new(PurgeScope::Text(live.clone())))
        .unwrap();
    assert_eq!(in_store(&f.store, &[&live]), Vec::<usize>::new());
    let n = s.take_notes(ed).unwrap();
    let o = n.notes[0].overwrite.as_ref().expect("the note stays");
    assert_eq!(o.rule, "dropped_col");
    assert!(o.withheld && o.yours.is_empty(), "{o:?}");
}

#[test]
fn grid_purge_by_text_leaves_no_bytes_behind() {
    let (f, s) = fx("text", BODY);
    let own = join(&s, "own");
    let ed = join(&s, "ed");
    let six = plant_six(&f, &s, ed);
    let all: Vec<&str> = six.iter().map(String::as_str).collect();
    // the negative control: the store holds all six, the file the live one
    assert_eq!(in_store(&f.store, &all), vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(in_file(&f.file, &all), vec![0]);
    let mut values = 0;
    for (n, t) in six.iter().enumerate() {
        let mut req = PurgeRequest::new(PurgeScope::Text(t.clone()));
        if n < 2 {
            req = req.and_remove();
        }
        let out = s.purge(own, req).unwrap();
        assert_eq!(out.live_matches, 0, "secret {n} still live");
        values += out.counts.values;
    }
    assert!(values >= 6, "{values} values redacted");
    assert_eq!(in_store(&f.store, &all), Vec::<usize>::new(), "store file");
    assert_eq!(in_file(&f.file, &all), Vec::<usize>::new(), "the file");
    // nothing live holds them, the hidden cell included; the rest is untouched
    for v in live_values(&s, ed) {
        assert!(all.iter().all(|x| !holds_window(&v, x)), "live: {v}");
    }
    assert_eq!(in_store(&f.store, &["plain"]), vec![0]);
    // restoring the hidden row brings back its cell without the secret
    op(&s, ed, |t| t.restore_row(&k("r3"))).unwrap();
    assert!(live_values(&s, ed)
        .iter()
        .all(|v| !holds_window(v, &six[1])));
    // the named version keeps its entry, its value gone, the redaction named
    let vs = s.versions(own).unwrap();
    let named = vs
        .iter()
        .find(|v| v.name.as_deref() == Some("with a secret"))
        .unwrap();
    assert!(
        named.redacted.iter().any(|r| r.root.starts_with("cells/")),
        "{:?}",
        named.redacted
    );
    let shown = format!("{:?}", s.show_content(own, named.id).unwrap());
    assert!(!holds_window(&shown, &six[3]));
}

/// The same six, the live and hidden values overwritten first, then one whole-history purge.
#[test]
fn grid_history_purge_leaves_no_bytes_behind() {
    let (f, s) = fx("history", BODY);
    let own = join(&s, "own");
    let ed = join(&s, "ed");
    let six = plant_six(&f, &s, ed);
    let all: Vec<&str> = six.iter().map(String::as_str).collect();
    // history only: the live cell and the hidden cell are overwritten (by key: the table
    // refuses a write into a deleted row, the kit's map doesn't)
    set(&s, ed, "r2", "c1", "gone");
    s.edit_ops(
        ed,
        vec![Edit::Map {
            root: table::CELLS.into(),
            ops: vec![MapOp::Set {
                key: table::cell_key(&k("r3"), &k("c2")),
                value: Value::str("gone too"),
            }],
        }],
        &EditOptions::new(),
    )
    .unwrap();
    assert_eq!(in_store(&f.store, &all), vec![0, 1, 2, 3, 4, 5]);
    let out = s
        .purge(own, PurgeRequest::new(PurgeScope::History))
        .unwrap();
    assert_eq!(out.live_matches, 0);
    assert!(out.counts.values > 0 && out.counts.log_entries > 0);
    assert!(out.rewrite.is_some());
    assert_eq!(in_store(&f.store, &all), Vec::<usize>::new(), "store file");
    assert_eq!(in_file(&f.file, &all), Vec::<usize>::new(), "the file");
    assert_eq!(in_store(&f.store, &["gone too"]), vec![0], "live kept");
}

/// Without `and_remove`, a live or hidden value stays (purge removes history) and is counted
/// in `live_matches`; with it, the value loses the text and keeps the rest.
#[test]
fn grid_and_remove_strips_live_and_hidden_values() {
    let (_f, s) = fx("strip", "a,b\nc,d\n");
    let own = join(&s, "own");
    let ed = join(&s, "ed");
    let t = secret();
    set(&s, ed, "r1", "c1", &format!("pre{t}post"));
    set(&s, ed, "r2", "c2", &t);
    op(&s, ed, |tb| tb.delete_row(&k("r2"))).unwrap();
    let out = s
        .purge(own, PurgeRequest::new(PurgeScope::Text(t.clone())))
        .unwrap();
    assert_eq!(out.live_matches, 2, "one live, one hidden");
    let out = s
        .purge(
            own,
            PurgeRequest::new(PurgeScope::Text(t.clone())).and_remove(),
        )
        .unwrap();
    assert!(out.removed.is_some());
    assert_eq!(out.live_matches, 0);
    let vals = live_values(&s, ed);
    assert!(vals.iter().any(|v| v == "prepost"), "{vals:?}");
    assert!(vals.iter().all(|v| !v.contains(&t)));
}

// ---------- scopes ----------

/// `--cell`: every value the cell held is redacted in records and versions, and a copy of one
/// in another cell too; a cell never holding them keeps its history; the live value stays.
/// `--row`: every cell of the row. Audited with keys only.
#[test]
fn grid_purge_cell_and_row_scopes() {
    let (f, s) = fx("scopes", "a,b\nc,d\ne,f\n");
    let own = join(&s, "own");
    let ed = join(&s, "ed");
    let (x1, x2, keep) = (secret(), secret(), secret());
    set(&s, ed, "r1", "c1", &x1);
    s.name_version(ed, "v", None).unwrap();
    set(&s, ed, "r1", "c1", &x2);
    set(&s, ed, "r3", "c2", &x1); // a copy elsewhere
    set(&s, ed, "r3", "c2", "z");
    set(&s, ed, "r2", "c1", &keep);
    set(&s, ed, "r2", "c1", "w");
    let out = s
        .purge(
            own,
            PurgeRequest::new(PurgeScope::Cell {
                row: k("r1"),
                col: k("c1"),
            }),
        )
        .unwrap();
    // x2 is live in r1/c1: it stays live, and is counted
    assert_eq!(out.live_matches, 1);
    assert_eq!(in_store(&f.store, &[&x1, &x2, &keep]), vec![1, 2]);
    assert!(f.host.events.lock().unwrap().iter().any(|e| matches!(
        e,
        AuditEvent::Purged { scope: PurgedScope::Cell { row, col }, .. } if row == "r1" && col == "c1"
    )));
    // --row r2: its cells' history goes (keep), live values stay
    s.purge(own, PurgeRequest::new(PurgeScope::Row { row: k("r2") }))
        .unwrap();
    assert_eq!(in_store(&f.store, &[&x1, &x2, &keep]), vec![1]);
    assert!(live_values(&s, ed).contains(&"w".to_owned()));
    // a text doc has no cells
    let md = f
        .mgr
        .open(
            Arc::new(k2_mpc_base::session::TextFilePack::new("markdown", 1 << 20)),
            &DocKey {
                space: "ws".into(),
                path: "n.md".into(),
            },
            Some(b"x\n"),
        )
        .unwrap();
    let o2 = join(&md, "own");
    assert!(matches!(
        md.purge(o2, PurgeRequest::new(PurgeScope::Row { row: k("r1") })),
        Err(SessionError::BadEdit(_))
    ));
}

/// A revert over a change whose value was purged reports `erased` and puts nothing back;
/// the undo stacks are cleared (U19).
#[test]
fn grid_revert_over_a_purged_value_reports_erased() {
    let (_f, s) = fx("revert", "a,b\nc,d\n");
    let own = join(&s, "own");
    let ed = join(&s, "ed");
    let x = secret();
    set(&s, ed, "r1", "c1", &x);
    let over = set(&s, ed, "r1", "c1", "y");
    s.purge(own, PurgeRequest::new(PurgeScope::Text(x.clone())))
        .unwrap();
    match s.revert(ed, over, false).unwrap() {
        UndoOutcome::Conflicts { conflicts, .. } => {
            assert!(
                conflicts.iter().any(|c| c.kind == ConflictKind::Erased),
                "{conflicts:?}"
            );
        }
        o => panic!("{o:?}"),
    }
    assert!(live_values(&s, ed).iter().all(|v| !v.contains(&x)));
    assert_eq!(s.undo(ed, 1).unwrap(), UndoOutcome::ClearedByPurge);
}

// ---------- purge then restore never resurrects ----------

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// One random history with a secret planted in it (whole, or in two halves in two cells),
/// then `purge --text --and-remove`, then every way back: each version restored, each change
/// reverted, undo and redo. Fails if the secret (any 8-byte window) is ever live again, in
/// the doc's file, or in the store file at the end.
fn one_history(seed: u64) -> (u64, u64, u64) {
    let mut rng = SmallRng::seed_from_u64(seed);
    let (f, s) = fx("pt", "h1,h2,h3\na,b,c\nd,e,f\ng,h,i\n");
    let own = join(&s, "own");
    let eds = [join(&s, "ed"), join(&s, "al")];
    // the owner's (an editor's undo can't turn it off again)
    header_on(&s, own);
    let t = secret();
    let (h1, h2) = t.split_at(24);
    let mut planted = false;
    let mut changes = Vec::new();
    let mut log = vec![format!("seed {seed}")];
    let steps = rng.gen_range(6..24);
    for step in 0..steps {
        let c = eds[rng.gen_range(0..2)];
        let tb = s.read(c).unwrap().content;
        let table = View::of_content(&tb).table().unwrap();
        let rows = table.live_rows();
        let cols = table.live_cols();
        let pick = rng.gen_range(0..100);
        // the secret goes in by step 3 at the latest
        let plant = !planted && (step >= 3 || rng.gen_bool(0.3));
        let r = if rows.is_empty() {
            None
        } else {
            Some(rows[rng.gen_range(0..rows.len())].clone())
        };
        let col = if cols.is_empty() {
            None
        } else {
            Some(cols[rng.gen_range(0..cols.len())].clone())
        };
        let res = match (pick, r, col) {
            (_, Some(r), Some(col)) if plant => {
                let res = match rng.gen_range(0..3) {
                    0 => {
                        log.push(format!("plant whole in {r}/{col}"));
                        try_op(&s, c, |tb| tb.set_cell(&r, &col, &t))
                    }
                    1 => {
                        log.push(format!("plant halves around {r}/{col}"));
                        let a = try_op(&s, c, |tb| tb.set_cell(&r, &col, h1));
                        let other = cols
                            [(cols.iter().position(|x| *x == col).unwrap() + 1) % cols.len()]
                        .clone();
                        let _ = a;
                        try_op(&s, c, |tb| tb.set_cell(&r, &other, h2))
                    }
                    _ => {
                        log.push(format!("plant in the name of {col}"));
                        try_op(&s, c, |tb| tb.rename_col(&col, &t))
                    }
                };
                // planted once a change holding it applied (a refused op plants nothing)
                planted = matches!(res, Ok(v) if v > 0);
                res
            }
            (0..=39, Some(r), Some(col)) => {
                let v = format!("v{}", rng.gen_range(0..1000));
                log.push(format!("set {r}/{col} {v}"));
                try_op(&s, c, |tb| tb.set_cell(&r, &col, &v))
            }
            (40..=49, Some(r), _) => {
                log.push(format!("delete row {r}"));
                try_op(&s, c, |tb| tb.delete_row(&r))
            }
            (50..=57, _, _) => {
                let n = cols.len().max(1);
                let vals: Vec<String> = (0..n).map(|i| format!("n{i}")).collect();
                log.push("insert row".into());
                try_op(&s, c, |tb| tb.insert_row(Anchor::End, &vals).map(|x| x.1))
            }
            (58..=63, _, Some(col)) => {
                log.push(format!("rename {col}"));
                try_op(&s, c, |tb| tb.rename_col(&col, "nm"))
            }
            (64..=71, _, _) => {
                log.push("name a version".into());
                s.name_version(c, &format!("v{step}"), None).map(|_| 0)
            }
            (72..=79, _, _) => {
                log.push("undo".into());
                s.undo(c, 1).map(|_| 0)
            }
            (80..=84, _, _) => {
                log.push("redo".into());
                s.redo(c, 1).map(|_| 0)
            }
            (85..=89, _, _) => {
                log.push("flush".into());
                s.flush(c).map(|_| 0)
            }
            (90..=94, _, _) => {
                // a deleted row back
                let tb = View::of_content(&s.read(c).unwrap().content)
                    .table()
                    .unwrap();
                let gone = tb.rows.deleted().next().cloned();
                match gone {
                    Some(r) => {
                        log.push(format!("restore row {r}"));
                        try_op(&s, c, |t| t.restore_row(&r))
                    }
                    None => Ok(0),
                }
            }
            _ => Ok(0),
        };
        if let Ok(v) = res {
            if v > 0 {
                changes.push(v);
            }
        }
    }
    let needles = [t.as_str()];
    let fail = |why: String, log: &[String]| -> ! { panic!("{why}\n  {}", log.join("\n  ")) };
    // the negative control: a planted secret is in the store before the purge
    if planted && in_store(&f.store, &needles).is_empty() {
        fail("planted, yet the store doesn't hold it".into(), &log);
    }
    // the purge
    let out = s
        .purge(
            own,
            PurgeRequest::new(PurgeScope::Text(t.clone())).and_remove(),
        )
        .unwrap();
    // halves are under the text: windows of it; and_remove strips whole copies only, so a
    // half still live is reported, and taken out here as an owner's edit
    let halves_live = live_values(&s, own).iter().any(|v| holds_window(v, &t));
    if halves_live {
        assert!(out.live_matches > 0, "a live half went unreported");
        clear_values(&s, own, &t);
        let again = s
            .purge(own, PurgeRequest::new(PurgeScope::Text(t.clone())))
            .unwrap();
        assert_eq!(again.live_matches, 0);
    }
    let check = |what: &str, log: &[String]| {
        for v in live_values(&s, own) {
            if holds_window(&v, &t) {
                fail(format!("{what}: the secret is live again: {v}"), log);
            }
        }
        if !in_file(&f.file, &needles).is_empty() {
            fail(format!("{what}: the file holds it"), log);
        }
    };
    check("after the purge", &log);
    // every way back
    let mut tries = 0u64;
    for v in s.versions(own).unwrap() {
        let _ = s.restore(own, v.id);
        tries += 1;
        check(&format!("restore {:?}", v.id), &log);
    }
    for c in changes.iter().rev() {
        let _ = s.revert(own, *c, rng.gen_bool(0.5));
        tries += 1;
        check(&format!("revert {c}"), &log);
    }
    for c in eds {
        let _ = s.undo(c, 3);
        let _ = s.redo(c, 3);
        tries += 2;
        check("undo/redo", &log);
    }
    s.flush(own).unwrap();
    check("the end", &log);
    if !in_store(&f.store, &needles).is_empty() {
        fail("the store file holds it".into(), &log);
    }
    (1, tries, u64::from(planted))
}

#[test]
fn purge_then_restore_never_resurrects() {
    let cases = env("GRID_ERASURE_CASES", 32);
    let seed = env("GRID_ERASURE_SEED", 0);
    let threads = env("GRID_ERASURE_THREADS", 1).max(1);
    let next = Arc::new(AtomicU64::new(0));
    let totals = Arc::new(Mutex::new((0u64, 0u64, 0u64)));
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let (next, totals) = (next.clone(), totals.clone());
            std::thread::spawn(move || loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= cases {
                    break;
                }
                let t0 = std::time::Instant::now();
                let (n, tries, planted) = one_history(seed + i);
                if std::env::var("GRID_ERASURE_VERBOSE").is_ok() {
                    eprintln!("[grid erasure] case {} in {:?}", seed + i, t0.elapsed());
                }
                let mut t = totals.lock().unwrap();
                t.0 += n;
                t.1 += tries;
                t.2 += planted;
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    let (n, tries, planted) = *totals.lock().unwrap();
    assert_eq!(n, cases);
    // the secret goes in by the fourth step whenever the table has a row and a column
    assert!(
        planted * 10 >= cases * 9,
        "{planted} of {cases} histories planted it"
    );
    eprintln!(
        "RUNREC {{\"job\":null,\"run\":\"grid_purge_restore\",\"tool\":\"grid_erasure\",\"status\":\"COMPLETE\",\"cases\":{n},\"planted\":{planted},\"ways_back\":{tries},\"seed\":{seed}}}"
    );
}

// ---------- the store rewrite on a grid doc (C1 merge with main, v0.9.5's table check) ----------

/// The rendered table as `c` reads it.
fn table_text(s: &Session, c: ConnId) -> String {
    String::from_utf8(s.read(c).unwrap().bytes).unwrap()
}

/// A purge's store rewrite on a grid doc, end to end: the doc holds every format-6 kind of
/// row (element writes, typed versions, version-3 change records from a revert and a
/// restore), so v0.9.5's check (a table missing from the rewrite's list is a newer kit's,
/// `MPC-E502 store_newer`) would refuse it if one were missing. After the rewrite, and
/// again on a fresh manager over the rewritten file, the doc reads the same, each cell's
/// blame (from the element writes) is the same, the versions and the log are the same,
/// and history still works: a revert over a change from before the rewrite applies.
#[test]
fn a_grid_purge_rewrite_keeps_every_table() {
    let (f, s) = fx("rewrite", BODY);
    let own = join(&s, "own");
    let ed = join(&s, "ed");
    header_on(&s, ed);
    set(&s, ed, "r2", "c1", "kept");
    let named = s.name_version(ed, "named", None).unwrap();
    let t = secret();
    set(&s, ed, "r3", "c2", &t);
    set(&s, ed, "r3", "c2", "over");
    let moved = op(&s, ed, |tb| tb.move_row(&k("r4"), Anchor::Start)).unwrap();
    let reverted = set(&s, own, "r2", "c2", "to revert");
    assert!(matches!(
        s.revert(own, reverted, false).unwrap(),
        UndoOutcome::Applied { .. }
    ));
    op(&s, ed, |tb| tb.rename_col(&k("c1"), "first")).unwrap();
    s.flush(ed).unwrap();
    f.store.check_rewritable().unwrap();

    let cells = [("r2", "c1"), ("r3", "c2"), ("r4", "c1"), ("r2", "c2")];
    let blames = |s: &Session, c: ConnId| -> Vec<Option<(String, u64)>> {
        cells
            .iter()
            .map(|(r, col)| {
                s.blame_cell(c, &k(r), &k(col))
                    .unwrap()
                    .map(|b| (b.author, b.change))
            })
            .collect()
    };
    let before = (
        table_text(&s, ed),
        blames(&s, ed),
        s.versions(ed).unwrap().len(),
        s.log(ed, &Default::default()).unwrap().len(),
    );
    assert!(before.1.iter().all(Option::is_some), "{:?}", before.1);

    let out = s
        .purge(own, PurgeRequest::new(PurgeScope::Text(t.clone())))
        .unwrap();
    let rw = out.rewrite.expect("the purge rewrote the store file");
    assert!(rw.rows > 0);
    assert_eq!(in_store(&f.store, &[&t]), Vec::<usize>::new(), "purged");
    f.store.check_rewritable().unwrap();
    let after = |s: &Session, c: ConnId| {
        (
            table_text(s, c),
            blames(s, c),
            s.versions(c).unwrap().len(),
            s.log(c, &Default::default()).unwrap().len(),
        )
    };
    assert_eq!(after(&s, ed), before, "the live session after the rewrite");
    assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
    assert_eq!(s.audit_disk_sync().unwrap(), vec![]);

    // a fresh manager over the rewritten file: everything was copied, nothing dropped
    drop(s);
    let mgr = SessionManager::new(f.store.clone(), f.host.clone(), config());
    let key = DocKey {
        space: "ws".into(),
        path: "t.csv".into(),
    };
    let s = mgr
        .open_file(Arc::new(Lite), &key, &f.file, f.file.parent().unwrap())
        .unwrap();
    let ed = join(&s, "ed");
    let own = join(&s, "own");
    assert_eq!(after(&s, ed), before, "reopened over the rewritten file");
    assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
    // history from before the rewrite still works: the move's record, and the named version
    assert!(matches!(
        s.revert(own, moved, false).unwrap(),
        UndoOutcome::Applied { .. }
    ));
    assert!(s.versions(ed).unwrap().iter().any(|v| v.id == named));
    let _ = &f.mgr;
}
