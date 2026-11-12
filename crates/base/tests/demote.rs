//! **Text → tier 0** (`SessionManager::demote_text_file`, `docs/SESSIONS.md`, "Text → tier
//! 0"): a live text doc becomes a tier-0 doc under the same key and doc id, its history kept in
//! place. Why: a pack can move a name to not-text (CAD, BIM and circuit-board files, whose lines
//! carry ids a line merge can corrupt), and a doc already stored as text under such a name must
//! not stay stuck (`open_blob_file` on it is `PackMismatch`).
//!
//! Each rule is a row (`fn row_*() -> Result<(), String>`, an `Err` naming the row):
//!
//! - nothing unwritten: every version and the live state carried exactly, the file untouched;
//! - an unwritten change: written to the file, and current;
//! - an outside save since the last sync: kept both ways (the save current, the live state a
//!   version and a conflict copy), never merged by lines;
//! - a change the file never got (closed so, as after a crash): kept, and written;
//! - after a restart: the tier-0 doc as it was left, the file in step;
//! - history: each version's bytes are what the text doc's `file_at` gave, names kept, and the
//!   change log (approvals included) readable as the live log;
//! - a second demotion is a no-op;
//! - a structured doc is refused typed (`Declined { code: "not_text" }`), another pack
//!   `PackMismatch`;
//! - the text session ends cleanly: roster `Closed { reason: "demoted" }` last, claims ended,
//!   `SessionClosed` and `Demoted` audited, later calls `Closed { reason: "demoted" }`;
//! - erasure and purge reach everything.
//!
//! The plants (`k2_mpc_base::differential::DemoteMutant`): `DropsUnwritten` (the tier-0 doc
//! starts from the file) turns the unwritten-change and crash rows red, `MergesByLine` (the
//! outside save taken in by lines first) the keep-both row, each by name.
//!
//! The N-1 checks (ignored; `tools/n1/n1.sh layout-demote` / `node-demote`) are at the end.

mod grid_pack;

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::{self, DemoteMutant};
use k2_mpc_base::session::{
    AnyFile, ApprovalState, AuditEvent, BlobLogKind, BlobSession, Config, ConnId, Demotion, Host,
    LineRange, LogItem, LogQuery, Participant, ParticipantKind, Rights, RosterEvent, Session,
    SessionError, SessionManager, Stay, TextFilePack, WriteOutcome, DEMOTED,
};
use k2_mpc_base::store::{scan_file, DocKey, DocStore, Finding, VersionId, BLOB_DOC_PACK};

const TXT: TextFilePack = TextFilePack::new("text", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;

// ---------- the harness ----------

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
    mgr: SessionManager,
}

fn config() -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c
}

fn open_at(dir: &Path, host: &Arc<TestHost>) -> (Arc<DocStore>, SessionManager) {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(store.clone(), host.clone(), config()).with_blob_store(blobs);
    (store, mgr)
}

fn fx_in(dir: PathBuf) -> Fx {
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_111_400_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let (store, mgr) = open_at(&dir, &host);
    let f = Fx {
        dir: TmpDir(dir),
        ws,
        host,
        store,
        mgr,
    };
    f.grant("own", Rights::full().with_owner(true));
    f.grant("ed", Rights::full());
    for a in ["agent-a", "agent-b", "agent-c"] {
        f.grant(a, Rights::editor());
    }
    f
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-demote-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fx_in(dir)
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(
        pid,
        pid.to_uppercase(),
        "#123456",
        ParticipantKind::Person,
        "tab",
    )
}

fn agent(pid: &str) -> Participant {
    Participant::new(pid, pid, "#654321", ParticipantKind::Agent, "cli")
}

impl Fx {
    fn grant(&self, pid: &str, r: Rights) {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn path(&self, name: &str) -> PathBuf {
        self.ws.join(name)
    }
    fn bytes(&self, name: &str) -> Vec<u8> {
        fs::read(self.path(name)).unwrap()
    }
    fn open_text(&self, name: &str) -> Arc<Session> {
        self.mgr
            .open_file(Arc::new(TXT), &key(name), &self.path(name), &self.ws)
            .unwrap()
    }
    fn demote(&self, name: &str) -> Result<(Arc<BlobSession>, Demotion), SessionError> {
        self.mgr.demote_text_file(
            Arc::new(TXT),
            Arc::new(AnyFile),
            &key(name),
            &self.path(name),
            &self.ws,
        )
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
    /// The process ends and starts again: the manager and the store are dropped, then opened
    /// on the same files.
    fn restart(self) -> Fx {
        let Fx {
            dir,
            ws,
            host,
            store,
            mgr,
        } = self;
        mgr.release().unwrap();
        drop(mgr);
        drop(store);
        let (store, mgr) = open_at(&dir.0, &host);
        Fx {
            dir,
            ws,
            host,
            store,
            mgr,
        }
    }
}

fn join(s: &Session, p: Participant) -> ConnId {
    s.join(p, Stay::Held).unwrap().conn
}

fn join_blob(s: &BlobSession, p: Participant) -> ConnId {
    s.join(p, Stay::Held).unwrap().conn
}

/// An agent's whole-file write from the version it read, which must apply.
fn write(s: &Session, c: ConnId, to: &str) {
    let v = s.read(c).unwrap().version;
    let out = s.write_base(c, ROOT, v, to).unwrap();
    assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
}

fn current(s: &BlobSession, c: ConnId) -> Vec<u8> {
    let mut b = Vec::new();
    s.read(c).unwrap().reader.read_to_end(&mut b).unwrap();
    b
}

fn at(s: &BlobSession, c: ConnId, v: VersionId) -> Vec<u8> {
    let mut b = Vec::new();
    s.read_version(c, v)
        .unwrap()
        .reader
        .read_to_end(&mut b)
        .unwrap();
    b
}

/// Every text version and the bytes its `file_at` gives, oldest first.
fn text_files(s: &Session, c: ConnId) -> Vec<(VersionId, Option<String>, Vec<u8>)> {
    s.versions(c)
        .unwrap()
        .into_iter()
        .map(|v| (v.id, v.name.clone(), s.file_at(c, v.id).unwrap().bytes))
        .collect()
}

/// The changes of a log, as (seq, author, kind, for whom, lines added, removed, approval).
type Row = (
    u64,
    String,
    &'static str,
    Option<String>,
    u64,
    u64,
    Option<ApprovalState>,
);

fn changes(items: Vec<LogItem>) -> Vec<Row> {
    items
        .into_iter()
        .filter_map(|i| match i {
            LogItem::Change { info, approval } => Some((
                info.seq,
                info.author,
                info.kind.as_str(),
                info.on_behalf_of,
                info.lines_added,
                info.lines_removed,
                approval,
            )),
            _ => None,
        })
        .collect()
}

/// `Err(row: what)` unless `ok`.
fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {}", what()))
    }
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

const IFC: &str = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n\
#1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'Project',$,$,$,$,$,$);\n\
#2=IFCWALL('2O2Fr$t4X7Zf8NOew3FLOH',$,'Wall A',$,$,$,$,$,$);\n\
ENDSEC;\nEND-ISO-10303-21;\n";
const WALL_2: &str = "#2=IFCWALL('2O2Fr$t4X7Zf8NOew3FLOH',$,'Wall A',$,$,$,$,$,$);\n";
const WALL_3: &str = "#3=IFCWALL('1hOSvn6df7F8_7GcBWlRGQ',$,'Wall B',$,$,$,$,$,$);\n";
const DOOR_4: &str = "#4=IFCDOOR('3cUkl32yn9qRSPvBJVyWYp',$,'Door 1',$,$,$,$,$,$);\n";
const WINDOW_4: &str = "#4=IFCWINDOW('0dWcU0d4LBqhgsE5zUPF0X',$,'Window 1',$,$,$,$,$,$);\n";

/// `base` with `line` added after the line `after`.
fn plus(base: &str, after: &str, line: &str) -> String {
    let i = base.find(after).expect("the anchor line") + after.len();
    format!("{}{line}{}", &base[..i], &base[i..])
}

fn with_3() -> String {
    plus(IFC, WALL_2, WALL_3)
}
fn with_door() -> String {
    plus(&with_3(), WALL_3, DOOR_4)
}
fn with_window() -> String {
    plus(&with_3(), WALL_3, WINDOW_4)
}

const NAME: &str = "model.ifc";

/// A `model.ifc` live as text: agent a's wall written to the file and a named version.
fn live_doc(f: &Fx) -> (Arc<Session>, ConnId, ConnId) {
    fs::write(f.path(NAME), IFC).unwrap();
    let s = f.open_text(NAME);
    let own = join(&s, person("own"));
    let a = join(&s, agent("agent-a"));
    f.advance(1_000);
    write(&s, a, &with_3());
    s.flush(a).unwrap();
    s.name_version(own, "wall b", None).unwrap();
    assert_eq!(f.bytes(NAME), with_3().as_bytes(), "a's wall is written");
    (s, own, a)
}

/// The checks every demotion passes: the versions map one to one with the same bytes as the
/// text doc's `file_at`, the text session is closed, later opens are tier 0.
#[allow(clippy::too_many_arguments)]
fn carried(
    row: &str,
    f: &Fx,
    old: &Session,
    old_conn: ConnId,
    before: &[(VersionId, Option<String>, Vec<u8>)],
    b: &BlobSession,
    c: ConnId,
    d: &Demotion,
) -> Result<(), String> {
    need(!d.already, row, || {
        "a first demotion reports already".into()
    })?;
    need(d.versions.len() == before.len(), row, || {
        format!("{} versions became {:?}", before.len(), d.versions)
    })?;
    for ((tv, name, bytes), (from, to)) in before.iter().zip(&d.versions) {
        need(tv == from, row, || format!("version {tv} mapped as {from}"))?;
        let got = at(b, c, *to);
        need(got == *bytes, row, || {
            format!(
                "version {tv} (now {to}) isn't its file_at:\n{}\nvs\n{}",
                show(&got),
                show(bytes)
            )
        })?;
        let v = b.versions(c).unwrap();
        let named = v
            .iter()
            .find(|(id, _)| id == to)
            .and_then(|(_, v)| v.name.clone());
        need(named == *name, row, || {
            format!("version {tv}'s name {name:?} became {named:?}")
        })?;
    }
    need(
        matches!(old.read(old_conn), Err(SessionError::Closed { reason }) if reason == DEMOTED),
        row,
        || "the text session isn't closed as demoted".into(),
    )?;
    match f
        .mgr
        .open_file(Arc::new(TXT), &key(NAME), &f.path(NAME), &f.ws)
    {
        Err(SessionError::PackMismatch { stored, .. }) if stored == BLOB_DOC_PACK => {}
        other => {
            return Err(format!(
                "{row}: a text open after the demotion: {:?}",
                other.map(|_| "opened as text")
            ))
        }
    }
    Ok(())
}

// ---------- the rows ----------

/// Nothing unwritten: the file holds the live state already. Every version and the live
/// state are carried exactly; the file isn't written.
fn row_demotes_with_no_unwritten_change() -> Result<(), String> {
    const ROW: &str = "row_demotes_with_no_unwritten_change";
    let f = fx("clean");
    let (s, own, _) = live_doc(&f);
    let before = text_files(&s, own);
    let mtime = fs::metadata(f.path(NAME)).unwrap().modified().unwrap();
    let (b, d) = f.demote(NAME).map_err(|e| format!("{ROW}: {e}"))?;
    let c = join_blob(&b, person("own"));
    carried(ROW, &f, &s, own, &before, &b, c, &d)?;
    need(!d.kept_both && !d.written, ROW, || format!("{d:?}"))?;
    need(d.current == d.live, ROW, || format!("{d:?}"))?;
    need(current(&b, c) == with_3().as_bytes(), ROW, || {
        format!("current is {}", show(&current(&b, c)))
    })?;
    need(f.bytes(NAME) == with_3().as_bytes(), ROW, || {
        "the file changed".into()
    })?;
    need(
        fs::metadata(f.path(NAME)).unwrap().modified().unwrap() == mtime,
        ROW,
        || "the file was rewritten".into(),
    )
}

#[test]
fn demotes_with_no_unwritten_change() {
    row_demotes_with_no_unwritten_change().unwrap();
}

/// An unwritten change (b's door, not written back yet): written to the file by the
/// demotion, and current.
fn row_demotes_with_an_unwritten_change() -> Result<(), String> {
    const ROW: &str = "row_demotes_with_an_unwritten_change";
    let f = fx("unwritten");
    let (s, own, _) = live_doc(&f);
    let bc = join(&s, agent("agent-b"));
    write(&s, bc, &with_door());
    assert_eq!(
        f.bytes(NAME),
        with_3().as_bytes(),
        "b's door isn't written yet"
    );
    let before = text_files(&s, own);
    let (b, d) = f.demote(NAME).map_err(|e| format!("{ROW}: {e}"))?;
    let c = join_blob(&b, person("own"));
    carried(ROW, &f, &s, own, &before, &b, c, &d)?;
    need(current(&b, c) == with_door().as_bytes(), ROW, || {
        format!("current is {}", show(&current(&b, c)))
    })?;
    need(f.bytes(NAME) == with_door().as_bytes(), ROW, || {
        format!("the file is {}", show(&f.bytes(NAME)))
    })?;
    need(
        d.written && !d.kept_both && d.current == d.live,
        ROW,
        || format!("{d:?}"),
    )
}

#[test]
fn demotes_with_an_unwritten_change() {
    row_demotes_with_an_unwritten_change().unwrap();
}

/// An outside save since the last sync (a window, saved by another program) while b's door
/// isn't written: tier 0's keep-both rule. The save is current and the file, exactly; the
/// live state is a version and a conflict copy; no version holds a line merge of the two.
fn row_outside_save_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_outside_save_keeps_both";
    let f = fx("outside");
    let (s, own, _) = live_doc(&f);
    let bc = join(&s, agent("agent-b"));
    write(&s, bc, &with_door());
    f.advance(2_000);
    let tmp = f.path(".app-save.tmp");
    fs::write(&tmp, with_window()).unwrap();
    fs::rename(&tmp, f.path(NAME)).unwrap();
    let before = text_files(&s, own);
    let (b, d) = f.demote(NAME).map_err(|e| format!("{ROW}: {e}"))?;
    let c = join_blob(&b, person("own"));
    carried(ROW, &f, &s, own, &before, &b, c, &d)?;
    need(d.kept_both, ROW, || format!("not kept both: {d:?}"))?;
    need(current(&b, c) == with_window().as_bytes(), ROW, || {
        format!("current isn't the save:\n{}", show(&current(&b, c)))
    })?;
    need(f.bytes(NAME) == with_window().as_bytes(), ROW, || {
        format!("the file isn't the save:\n{}", show(&f.bytes(NAME)))
    })?;
    need(at(&b, c, d.live) == with_door().as_bytes(), ROW, || {
        format!("the live state's version is {}", show(&at(&b, c, d.live)))
    })?;
    let copy = d
        .conflict_copy
        .clone()
        .ok_or(format!("{ROW}: no conflict copy"))?;
    need(
        fs::read(&copy).ok() == Some(with_door().into_bytes()),
        ROW,
        || format!("the conflict copy {} isn't the live state", copy.display()),
    )?;
    for (v, _) in b.versions(c).unwrap() {
        let t = show(&at(&b, c, v));
        need(
            !(t.contains("IFCDOOR") && t.contains("IFCWINDOW")),
            ROW,
            || format!("version {v} merges the save by lines:\n{t}"),
        )?;
    }
    let log = b.log(c).unwrap();
    need(
        log.last()
            .is_some_and(|e| e.kind == BlobLogKind::ConflictKept),
        ROW,
        || format!("the save logs as {:?}", log.last().map(|e| e.kind)),
    )?;
    need(
        f.events().iter().any(
            |e| matches!(e, AuditEvent::BlobConflict { current, .. } if *current == d.current),
        ),
        ROW,
        || "no BlobConflict audited".into(),
    )
}

#[test]
fn outside_save_keeps_both() {
    row_outside_save_keeps_both().unwrap();
}

/// A doc closed with a change the file never got (b's door, made without the file; a crash
/// before write-back leaves the same): the change is current and written.
fn row_crash_change_is_kept() -> Result<(), String> {
    const ROW: &str = "row_crash_change_is_kept";
    let f = fx("crash");
    let (s, _, _) = live_doc(&f);
    drop(s);
    assert!(f.mgr.close(&key(NAME), "shutdown").unwrap());
    let t = f.mgr.open(Arc::new(TXT), &key(NAME), None).unwrap();
    let bc = join(&t, agent("agent-b"));
    write(&t, bc, &with_door());
    drop(t);
    assert!(f.mgr.close(&key(NAME), "crash").unwrap());
    assert_eq!(
        f.bytes(NAME),
        with_3().as_bytes(),
        "the file never got the door"
    );
    let t = f.mgr.open(Arc::new(TXT), &key(NAME), None).unwrap();
    let own = join(&t, person("own"));
    let before = text_files(&t, own);
    drop(t);
    let (b, d) = f.demote(NAME).map_err(|e| format!("{ROW}: {e}"))?;
    let c = join_blob(&b, person("own"));
    need(d.versions.len() == before.len(), ROW, || format!("{d:?}"))?;
    need(current(&b, c) == with_door().as_bytes(), ROW, || {
        format!("current is {}", show(&current(&b, c)))
    })?;
    need(f.bytes(NAME) == with_door().as_bytes(), ROW, || {
        format!("the file is {}", show(&f.bytes(NAME)))
    })?;
    need(d.written && !d.kept_both, ROW, || format!("{d:?}"))
}

#[test]
fn crash_change_is_kept() {
    row_crash_change_is_kept().unwrap();
}

/// After a restart the tier-0 doc is as the demotion left it: the same versions, current and
/// live log; the file in step (the first tick takes no outside save); text opens refused.
fn row_after_restart() -> Result<(), String> {
    const ROW: &str = "row_after_restart";
    let f = fx("restart");
    let (s, _, _) = live_doc(&f);
    let bc = join(&s, agent("agent-b"));
    write(&s, bc, &with_door());
    drop(s);
    let (b, d) = f.demote(NAME).map_err(|e| format!("{ROW}: {e}"))?;
    let c = join_blob(&b, person("own"));
    let versions = b.versions(c).unwrap();
    let live = b.live_log(c).unwrap();
    drop(b);
    let f = f.restart();
    let b = f
        .mgr
        .open_blob_file(Arc::new(AnyFile), &key(NAME), &f.path(NAME), &f.ws)
        .map_err(|e| format!("{ROW}: reopen: {e}"))?;
    let c = join_blob(&b, person("own"));
    need(b.versions(c).unwrap() == versions, ROW, || {
        "the versions changed".into()
    })?;
    need(b.live_log(c).unwrap() == live, ROW, || {
        "the live log changed".into()
    })?;
    need(b.status().unwrap().version == d.current, ROW, || {
        format!("current {} vs {}", b.status().unwrap().version, d.current)
    })?;
    need(current(&b, c) == with_door().as_bytes(), ROW, || {
        "current changed".into()
    })?;
    f.advance(5_000);
    let t = f.mgr.tick().unwrap();
    need(t.ingested.is_empty() && t.conflicts.is_empty(), ROW, || {
        format!("the first tick took the file in: {t:?}")
    })?;
    match f.mgr.open(Arc::new(TXT), &key(NAME), None) {
        Err(SessionError::PackMismatch { stored, .. }) if stored == BLOB_DOC_PACK => Ok(()),
        other => Err(format!("{ROW}: a text open: {:?}", other.map(|_| ()))),
    }
}

#[test]
fn after_restart() {
    row_after_restart().unwrap();
}

/// History: each version's bytes are what `file_at` gave, names kept; the change log, with an
/// on-behalf change's pending approval, reads back as the live log.
fn row_history_readable_and_file_at_exact() -> Result<(), String> {
    const ROW: &str = "row_history_readable_and_file_at_exact";
    let f = fx("history");
    let (s, own, _) = live_doc(&f);
    let cc = join(&s, agent("agent-c").on_behalf_of("own"));
    f.advance(1_000);
    write(&s, cc, &with_door());
    s.name_version(own, "door", None).unwrap();
    f.advance(1_000);
    let ed = join(&s, person("ed"));
    write(&s, ed, &with_door().replace("Door 1", "Front door"));
    let before = text_files(&s, own);
    let log = changes(s.log(own, &LogQuery::default()).unwrap());
    need(
        log.iter()
            .any(|r| r.3.as_deref() == Some("own") && r.6 == Some(ApprovalState::Pending)),
        ROW,
        || format!("control: no pending on-behalf change in {log:?}"),
    )?;
    let (b, d) = f.demote(NAME).map_err(|e| format!("{ROW}: {e}"))?;
    let c = join_blob(&b, person("own"));
    carried(ROW, &f, &s, own, &before, &b, c, &d)?;
    let live = changes(b.live_log(c).unwrap());
    need(live == log, ROW, || {
        format!("the live log\n{live:?}\nisn't the text log\n{log:?}")
    })?;
    need(d.changes == log.len() as u64, ROW, || format!("{d:?}"))?;
    need(
        current(&b, c) == with_door().replace("Door 1", "Front door").as_bytes(),
        ROW,
        || "current isn't the live state".into(),
    )
}

#[test]
fn history_readable_and_file_at_exact() {
    row_history_readable_and_file_at_exact().unwrap();
}

/// A second demotion is a no-op: `already`, nothing changed.
fn row_second_demote_is_a_noop() -> Result<(), String> {
    const ROW: &str = "row_second_demote_is_a_noop";
    let f = fx("twice");
    let (s, _, _) = live_doc(&f);
    drop(s);
    let (b, d) = f.demote(NAME).map_err(|e| format!("{ROW}: {e}"))?;
    let c = join_blob(&b, person("own"));
    let versions = b.versions(c).unwrap();
    let (b2, d2) = f.demote(NAME).map_err(|e| format!("{ROW}: again: {e}"))?;
    need(d2.already && d2.versions.is_empty(), ROW, || {
        format!("{d2:?}")
    })?;
    need(Arc::ptr_eq(&b, &b2), ROW, || "another session".into())?;
    need(b.versions(c).unwrap() == versions, ROW, || {
        "the versions changed".into()
    })?;
    need(b.status().unwrap().version == d.current, ROW, || {
        "current moved".into()
    })?;
    // without the file too
    let (_, d3) = f
        .mgr
        .demote_text(Arc::new(TXT), Arc::new(AnyFile), &key(NAME))
        .map_err(|e| format!("{ROW}: demote_text: {e}"))?;
    need(d3.already, ROW, || format!("{d3:?}"))
}

#[test]
fn second_demote_is_a_noop() {
    row_second_demote_is_a_noop().unwrap();
}

/// A structured doc is refused typed (`not_text`), a doc of another pack `PackMismatch`, a doc
/// in a file through `demote_text` `BadPath`, and nothing changes.
fn row_non_text_pack_refuses_typed() -> Result<(), String> {
    const ROW: &str = "row_non_text_pack_refuses_typed";
    let f = fx("refuse");
    let grid = "plan.csv";
    fs::write(f.path(grid), "a,b\n1,2\n").unwrap();
    let pack: Arc<dyn k2_mpc_base::session::Pack> = Arc::new(grid_pack::GridLite);
    let g = f
        .mgr
        .open_file(pack.clone(), &key(grid), &f.path(grid), &f.ws)
        .unwrap();
    drop(g);
    match f.mgr.demote_text_file(
        pack.clone(),
        Arc::new(AnyFile),
        &key(grid),
        &f.path(grid),
        &f.ws,
    ) {
        Err(SessionError::Declined(d)) if d.code == "not_text" => {}
        other => return Err(format!("{ROW}: a grid doc: {:?}", other.map(|r| r.1))),
    }
    need(f.mgr.open(pack, &key(grid), None).is_ok(), ROW, || {
        "the grid doc doesn't open as it was".into()
    })?;
    let (s, _, _) = live_doc(&f);
    drop(s);
    match f.mgr.demote_text_file(
        Arc::new(TextFilePack::new("markdown", 4 << 20)),
        Arc::new(AnyFile),
        &key(NAME),
        &f.path(NAME),
        &f.ws,
    ) {
        Err(SessionError::PackMismatch { stored, .. }) if stored == "text" => {}
        other => return Err(format!("{ROW}: another pack: {:?}", other.map(|r| r.1))),
    }
    match f
        .mgr
        .demote_text(Arc::new(TXT), Arc::new(AnyFile), &key(NAME))
    {
        Err(SessionError::BadPath(_)) => {}
        other => return Err(format!("{ROW}: without its file: {:?}", other.map(|r| r.1))),
    }
    let t = f.mgr.open(Arc::new(TXT), &key(NAME), None);
    need(t.is_ok(), ROW, || {
        "the text doc doesn't open as it was".into()
    })
}

#[test]
fn non_text_pack_refuses_typed() {
    row_non_text_pack_refuses_typed().unwrap();
}

/// The text session ends cleanly: the roster's last event is `Closed { reason: "demoted" }`,
/// a live claim ends (`left`), `SessionClosed` and `Demoted` name the connections, and later
/// calls fail with `Closed { reason: "demoted" }`.
fn row_session_ends_with_typed_events() -> Result<(), String> {
    const ROW: &str = "row_session_ends_with_typed_events";
    let f = fx("events");
    let (s, own, a) = live_doc(&f);
    let rx = s.subscribe_roster(own).unwrap();
    s.claim(
        a,
        LineRange {
            root: ROOT.into(),
            first: 6,
            last: 7,
        },
        None,
        Some("walls".into()),
        false,
    )
    .map_err(|e| format!("{ROW}: control: the claim: {e}"))?;
    let (_, d) = f.demote(NAME).map_err(|e| format!("{ROW}: {e}"))?;
    let got: Vec<RosterEvent> = rx.try_iter().collect();
    need(
        matches!(got.last(), Some(RosterEvent::Closed { reason }) if *reason == DEMOTED),
        ROW,
        || format!("the roster's last event: {:?}", got.last()),
    )?;
    let ev = f.events();
    need(
        ev.iter()
            .any(|e| matches!(e, AuditEvent::ClaimEnded { pid, .. } if pid == "agent-a")),
        ROW,
        || "the claim didn't end".into(),
    )?;
    let conns =
        |c: &[(ConnId, String)]| c.iter().any(|(x, _)| *x == a) && c.iter().any(|(x, _)| *x == own);
    need(
        ev.iter().any(|e| matches!(e, AuditEvent::SessionClosed { reason, connections, .. } if *reason == DEMOTED && conns(connections))),
        ROW,
        || "no SessionClosed (demoted) naming the connections".into(),
    )?;
    need(
        ev.iter().any(|e| matches!(e, AuditEvent::Demoted { connections, kept_both: false, .. } if conns(connections))),
        ROW,
        || "no Demoted naming the connections".into(),
    )?;
    need(conns(&d.connections), ROW, || {
        format!("{:?}", d.connections)
    })?;
    for (what, r) in [
        ("read", s.read(a).map(|_| ())),
        ("flush", s.flush(a).map(|_| ())),
        ("write", s.write_base(a, ROOT, 1, "x\n").map(|_| ())),
    ] {
        need(
            matches!(r, Err(SessionError::Closed { reason }) if reason == DEMOTED),
            ROW,
            || format!("{what} after the demotion: {r:?}"),
        )?;
    }
    Ok(())
}

#[test]
fn session_ends_with_typed_events() {
    row_session_ends_with_typed_events().unwrap();
}

/// Which of `needles` the store file holds (whole or by any 8-byte window).
fn in_store(store: &DocStore, needles: &[&str]) -> Vec<usize> {
    let b: Vec<&[u8]> = needles.iter().map(|s| s.as_bytes()).collect();
    hit(&store.scan_for(&b).unwrap())
}

fn hit(f: &[Finding]) -> Vec<usize> {
    let mut v: Vec<usize> = f.iter().map(|f| f.needle).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// Whether a file under the blob folder holds `needle` (whole or by any 8-byte window).
fn blobs_hold(f: &Fx, needle: &str) -> bool {
    files_under(&f.dir.0.join("blobs"))
        .iter()
        .any(|p| !hit(&scan_file(p, &[needle.as_bytes()]).unwrap()).is_empty())
}

/// Erasure and purge reach everything a demotion kept: a secret typed and removed while live
/// (in a version and the old log) is gone from the store file and the blobs after a tier-0
/// purge of the history; an erased agent's pid is gone from the store (versions, live log).
fn row_erasure_and_purge_reach_everything() -> Result<(), String> {
    const ROW: &str = "row_erasure_and_purge_reach_everything";
    let f = fx("erase");
    let (s, own, a) = live_doc(&f);
    let secret: String = {
        let b: [u8; 24] = rand::random();
        b.iter().map(|x| format!("{x:02x}")).collect()
    };
    write(
        &s,
        a,
        &plus(&with_3(), WALL_3, &format!("/* {secret} */\n")),
    );
    s.name_version(own, "with the secret", None).unwrap();
    write(&s, a, &with_3());
    s.flush(a).unwrap();
    drop(s);
    let (b, _) = f.demote(NAME).map_err(|e| format!("{ROW}: {e}"))?;
    let c = join_blob(&b, person("own"));
    need(blobs_hold(&f, &secret), ROW, || {
        "control: no blob holds the secret".into()
    })?;
    need(!in_store(&f.store, &["agent-a"]).is_empty(), ROW, || {
        "control: the store doesn't name the agent".into()
    })?;
    let out = b.purge(c, None).map_err(|e| format!("{ROW}: purge: {e}"))?;
    need(out.counts.blob_versions > 0, ROW, || format!("{out:?}"))?;
    need(!blobs_hold(&f, &secret), ROW, || {
        "a blob still holds the secret".into()
    })?;
    need(in_store(&f.store, &[&secret]).is_empty(), ROW, || {
        "the store file still holds the secret".into()
    })?;
    b.erase_participant(c, "agent-a", "erased-1")
        .map_err(|e| format!("{ROW}: erasure: {e}"))?;
    need(in_store(&f.store, &["agent-a"]).is_empty(), ROW, || {
        "the store still names the erased agent".into()
    })?;
    let live = changes(b.live_log(c).unwrap());
    need(live.iter().any(|r| r.1 == "erased-1"), ROW, || {
        format!("the live log doesn't show the pseudonym: {live:?}")
    })
}

#[test]
fn erasure_and_purge_reach_everything() {
    row_erasure_and_purge_reach_everything().unwrap();
}

// ---------- the plants ----------

type RowFn = fn() -> Result<(), String>;

fn red_by_name(mutant: DemoteMutant, rows: &[(&str, RowFn)]) {
    for (name, row) in rows {
        let out = differential::with_demote_mutant(mutant, row);
        match out {
            Err(e) => assert!(
                e.starts_with(name),
                "{mutant:?}: {name} failed elsewhere: {e}"
            ),
            Ok(()) => panic!("{mutant:?}: {name} stays green"),
        }
    }
}

/// "demote drops unwritten": the unwritten-change and crash rows go red, by name.
#[test]
fn the_planted_drops_unwritten_is_caught_by_name() {
    red_by_name(
        DemoteMutant::DropsUnwritten,
        &[
            (
                "row_demotes_with_an_unwritten_change",
                row_demotes_with_an_unwritten_change,
            ),
            ("row_crash_change_is_kept", row_crash_change_is_kept),
        ],
    );
}

/// "demote merges by line": the keep-both row goes red, by name.
#[test]
fn the_planted_merges_by_line_is_caught_by_name() {
    red_by_name(
        DemoteMutant::MergesByLine,
        &[("row_outside_save_keeps_both", row_outside_save_keeps_both)],
    );
}

// ---------- N-1 (tools/n1/n1.sh layout-demote / node-demote) ----------

/// `K2MB_N1_OUT`, the N-1 run's folder.
fn n1_dir() -> PathBuf {
    PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"))
}

/// The N-1 fixture: a fixed folder (its store, blobs and ws), kept after the run.
fn n1_fx(dir: PathBuf) -> Fx {
    fs::create_dir_all(&dir).unwrap();
    fx_in(dir)
}

/// Forward, this build's half: `<out>/fwd` holds a store with two demoted docs (`model.ifc`
/// with an outside save kept both, `gone.ifc`) and `expect.txt`, what the previous release
/// must read: one line per version, `<doc> <version> <hex of its bytes>`, and `<doc> current
/// <version>`.
#[test]
#[ignore = "N-1: tools/n1/n1.sh node-demote"]
fn n1_demote_producer() {
    let out = n1_dir().join("fwd");
    let _ = fs::remove_dir_all(&out);
    let f = n1_fx(out.clone());
    let mut expect = String::new();
    for name in ["model.ifc", "gone.ifc"] {
        fs::write(f.path(name), IFC).unwrap();
        let s = f.open_text(name);
        let a = join(&s, agent("agent-a"));
        write(&s, a, &with_3());
        s.flush(a).unwrap();
        write(&s, a, &with_door());
        if name == "model.ifc" {
            f.advance(2_000);
            fs::write(f.path(name), with_window()).unwrap();
        }
        drop(s);
        let (b, d) = f
            .mgr
            .demote_text_file(
                Arc::new(TXT),
                Arc::new(AnyFile),
                &key(name),
                &f.path(name),
                &f.ws,
            )
            .unwrap();
        let c = join_blob(&b, person("own"));
        for (v, _) in b.versions(c).unwrap() {
            let hex: String = at(&b, c, v).iter().map(|x| format!("{x:02x}")).collect();
            expect.push_str(&format!("{name} {v} {hex}\n"));
        }
        expect.push_str(&format!("{name} current {}\n", d.current));
        assert!(!b.live_log(c).unwrap().is_empty());
    }
    let gone = f.store.find(&key("gone.ifc")).unwrap().unwrap();
    fs::write(out.join("gone-id.txt"), gone.to_string()).unwrap();
    let f = f.restart();
    f.mgr.release().unwrap();
    fs::write(out.join("expect.txt"), expect).unwrap();
    std::mem::forget(f.dir); // the run's folder stays for the consumer
    println!("N-1 DEMOTE PRODUCED");
}

/// Forward, after the previous release ran on `<out>/fwd` (it read both docs at tier 0, wrote
/// a version to `model.ifc`, deleted `gone.ifc` and had its purge refused): this build reads
/// `model.ifc`'s live log and the other release's version, finds `gone.ifc`'s live-log rows
/// swept by its open, and purges.
#[test]
#[ignore = "N-1: tools/n1/n1.sh node-demote"]
fn n1_demote_after() {
    let out = n1_dir().join("fwd");
    // the deleted doc's id (the previous release can't read its live-log rows: an unknown
    // table, which its refused rewrite named)
    let gone_rows: Vec<u64> = fs::read_to_string(out.join("gone-id.txt"))
        .unwrap()
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    let f = n1_fx(out.clone());
    for id in &gone_rows {
        assert!(
            f.store.demoted_changes(*id).unwrap().is_empty(),
            "the deleted doc's live log is swept by this build's open"
        );
    }
    let b = f
        .mgr
        .open_blob_file(
            Arc::new(AnyFile),
            &key("model.ifc"),
            &f.path("model.ifc"),
            &f.ws,
        )
        .unwrap();
    let c = join_blob(&b, person("own"));
    assert!(
        !b.live_log(c).unwrap().is_empty(),
        "the live log is still there"
    );
    assert_eq!(
        current(&b, c),
        b"written by the previous release\n",
        "the previous release's version is current"
    );
    let purged = b.purge(c, None).unwrap();
    assert!(purged.counts.blob_versions > 0);
    f.mgr.release().unwrap();
    std::mem::forget(f.dir);
    println!("N-1 DEMOTE AFTER OK");
}

/// Reverse: the previous release wrote `<out>/rev` (a text doc, `model.ifc`, with a change the
/// file never got and an outside save made after); this build demotes it: the save is current,
/// the previous release's live state a version and a conflict copy, its change log the live
/// log.
#[test]
#[ignore = "N-1: tools/n1/n1.sh node-demote"]
fn n1_demote_prev_store() {
    let out = n1_dir().join("rev");
    let f = n1_fx(out.clone());
    let name = "model.ifc";
    let save = fs::read(f.path(name)).unwrap();
    let live = fs::read(out.join("live.txt")).unwrap();
    let (b, d) = f
        .mgr
        .demote_text_file(
            Arc::new(TXT),
            Arc::new(AnyFile),
            &key(name),
            &f.path(name),
            &f.ws,
        )
        .unwrap();
    let c = join_blob(&b, person("own"));
    assert!(d.kept_both, "{d:?}");
    assert_eq!(current(&b, c), save, "the outside save is current");
    assert_eq!(
        at(&b, c, d.live),
        live,
        "the previous release's live state is kept"
    );
    assert_eq!(fs::read(d.conflict_copy.unwrap()).unwrap(), live);
    assert!(
        !b.live_log(c).unwrap().is_empty(),
        "its change log is the live log"
    );
    f.mgr.release().unwrap();
    std::mem::forget(f.dir);
    println!("N-1 DEMOTE REV OK");
}
