//! CAD, BIM and circuit-board files are never live text: their lines carry ids and references
//! (`#12=` entities, handles, uuids, vertex numbers), so a whole-line merge can write duplicate
//! ids or leave references pointing at the wrong things while the file still parses. Until each
//! type has its own pack they open at tier 0 (a shared file with versions, and keep-both on
//! clashing saves).
//!
//! Each row takes the routing as a parameter ([`Route`]): the pack's table ([`Table`]), or the
//! plant [`CadExtensionLive`], the routing before these extensions were not-text (each of them
//! opened as live text of unknown type). The plant turns each CAD row red by name.
//!
//! A doc already live as text when the routing changes keeps all its content: the host demotes
//! it (`SessionManager::demote_text_file`), once, and opens the file at tier 0 as usual. The
//! same key becomes a tier-0 doc with every text version, the last live state current and
//! written to the file (an outside save since the last sync keeps both), and the change log
//! readable (`BlobSession::live_log`).

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, BlobSession, Config, ConnId, Demotion, Host, LogItem, LogQuery, Pack,
    Participant, ParticipantKind, Rights, Session, SessionError, SessionManager, Stay,
    WriteOutcome, DEMOTED,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionId, BLOB_DOC_PACK};
use k2_mpc_text::{TextPack, PACK, ROOT};

// ---------- the routing ----------

/// Which pack a host opens a file name with: `None` is tier 0.
trait Route {
    fn pack(&self, name: &str) -> Option<TextPack>;
}

/// The pack's table: [`TextPack::for_path`].
struct Table;

impl Route for Table {
    fn pack(&self, name: &str) -> Option<TextPack> {
        TextPack::for_path(name)
    }
}

/// The plant: the routing before this change. These extensions were in no group and not in
/// `NOT_TEXT`, so each opened as live text of unknown type ([`PACK`]).
struct CadExtensionLive;

impl Route for CadExtensionLive {
    fn pack(&self, name: &str) -> Option<TextPack> {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        match ext {
            Some(e) if CAD.contains(&e.as_str()) => Some(PACK),
            _ => TextPack::for_path(name),
        }
    }
}

/// The extensions that must never be live text, lowercase.
const CAD: &[&str] = &[
    "ifc",
    "ifczip",
    "step",
    "stp",
    "p21",
    "dxf",
    "kicad_sch",
    "kicad_pcb",
    "kicad_sym",
    "kicad_mod",
    "iges",
    "igs",
    "obj",
];

/// `Err(row: what)` unless `ok`.
fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {}", what()))
    }
}

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

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mt-eng-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
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
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    Fx { dir, ws, host, mgr }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

impl Fx {
    fn path(&self, name: &str) -> PathBuf {
        self.ws.join(name)
    }
    fn bytes(&self, name: &str) -> Vec<u8> {
        fs::read(self.path(name)).unwrap()
    }
    fn who(&self, pid: &str) -> Participant {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::full());
        Participant::new(pid, pid, "#123456", ParticipantKind::Agent, "cli")
    }
    fn join(&self, s: &Session, pid: &str) -> ConnId {
        s.join(self.who(pid), Stay::Held).unwrap().conn
    }
    fn join_blob(&self, s: &BlobSession, pid: &str) -> ConnId {
        s.join(self.who(pid), Stay::Held).unwrap().conn
    }
}

/// An agent's whole-file write from the version it read, which must apply.
fn write(s: &Session, c: ConnId, to: &str) {
    let v = s.read(c).unwrap().version;
    let out = s.write_base(c, ROOT, v, to).unwrap();
    assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
}

// ---------- the rows ----------

/// Every extension, in any case and under a folder, is never text.
fn cad_row(route: &dyn Route) -> Result<(), String> {
    const ROW: &str = "each_cad_extension_is_never_text";
    for e in CAD {
        for name in [
            format!("model.{e}"),
            format!("plans/Level 2.{}", e.to_ascii_uppercase()),
            format!("a.b.{e}"),
        ] {
            need(route.pack(&name).is_none(), ROW, || {
                format!("{name} opens as live text")
            })?;
        }
    }
    Ok(())
}

#[test]
fn each_cad_extension_is_never_text() {
    cad_row(&Table).unwrap();
}

/// Names that only contain one of these extensions stay what they were.
#[test]
fn a_name_that_only_contains_one_stays_text() {
    for name in [
        "notes.step.txt",
        "step",
        "obj",
        "dxf.md.txt",
        "ifc-export.log",
    ] {
        assert!(Table.pack(name).is_some(), "{name}");
    }
}

/// G-code is a line program with no ids or references between lines: it stays live text
/// (unknown type, the strict guard), and so does an ASCII STL, whose facets are self-contained
/// (no ids, no references).
#[test]
fn gcode_and_ascii_stl_stay_live_text() {
    for name in ["part.gcode", "Part.GCODE", "mesh.stl"] {
        assert_eq!(Table.pack(name), Some(PACK), "{name}");
    }
    let f = fx("gcode");
    let name = "part.gcode";
    fs::write(f.path(name), "G21\nG90\nG1 X10 Y10 F1500\nM2\n").unwrap();
    let pack: Arc<dyn Pack> = Arc::new(Table.pack(name).unwrap());
    let s = f
        .mgr
        .open_file(pack, &key(name), &f.path(name), &f.ws)
        .unwrap();
    let c = f.join(&s, "agent");
    write(&s, c, "G21\nG90\nG1 X10 Y10 F1500\nG1 X20 Y10\nM2\n");
    s.flush(c).unwrap();
    assert_eq!(
        f.bytes(name),
        b"G21\nG90\nG1 X10 Y10 F1500\nG1 X20 Y10\nM2\n"
    );
}

const IFC: &str = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n\
#1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'Project',$,$,$,$,$,$);\n\
#2=IFCWALL('2O2Fr$t4X7Zf8NOew3FLOH',$,'Wall A',$,$,$,$,$,$);\n\
ENDSEC;\nEND-ISO-10303-21;\n";

/// `base` with the line `line` added after `after`.
fn ifc_plus(base: &str, after: &str, line: &str) -> String {
    let at = base.find(after).expect("the anchor line") + after.len();
    format!("{}{line}\n{}", &base[..at], &base[at..])
}

const WALL_2: &str = "#2=IFCWALL('2O2Fr$t4X7Zf8NOew3FLOH',$,'Wall A',$,$,$,$,$,$);\n";
const WALL_3: &str = "#3=IFCWALL('1hOSvn6df7F8_7GcBWlRGQ',$,'Wall B',$,$,$,$,$,$);";
const DOOR_4: &str = "#4=IFCDOOR('3cUkl32yn9qRSPvBJVyWYp',$,'Door 1',$,$,$,$,$,$);";

const WINDOW_4: &str = "#4=IFCWINDOW('0dWcU0d4LBqhgsE5zUPF0X',$,'Window 1',$,$,$,$,$,$);";

/// How the text doc was left when the routing changed.
#[derive(Clone, Copy, Debug)]
enum Left {
    /// Open, with members, and a change not yet written back to the file.
    OpenWithUnwrittenChange,
    /// Closed (the host's shutdown, or idle unload): every change written back.
    Closed,
    /// Closed, with a change the file never got (the doc changed without its file: a crash
    /// before write-back leaves the same).
    ClosedWithChangeTheFileNeverGot,
    /// Open with a change not yet written back, and another program saved the file since.
    OpenWithOutsideSave,
}

impl Left {
    /// The demotion writes the live state to the file: the file doesn't hold it yet and no
    /// other program saved the file since.
    fn writes(self) -> bool {
        matches!(
            self,
            Left::OpenWithUnwrittenChange | Left::ClosedWithChangeTheFileNeverGot
        )
    }
}

/// Every text version and the bytes its `file_at` gives, oldest first.
fn text_files(s: &Session, c: ConnId) -> Vec<(VersionId, Vec<u8>)> {
    s.versions(c)
        .unwrap()
        .into_iter()
        .map(|v| (v.id, s.file_at(c, v.id).unwrap().bytes))
        .collect()
}

fn blob_at(b: &BlobSession, c: ConnId, v: VersionId) -> Vec<u8> {
    let mut out = Vec::new();
    b.read_version(c, v)
        .unwrap()
        .reader
        .read_to_end(&mut out)
        .unwrap();
    out
}

fn blob_current(b: &BlobSession, c: ConnId) -> Vec<u8> {
    let mut out = Vec::new();
    b.read(c).unwrap().reader.read_to_end(&mut out).unwrap();
    out
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The host's rule for a text doc whose name the pack now calls not-text: the kit's demote,
/// once, with the pack the doc is stored under (`stored` of the `PackMismatch`, here `text`).
fn demote(f: &Fx, name: &str) -> Result<(Arc<BlobSession>, Demotion), SessionError> {
    f.mgr.demote_text_file(
        Arc::new(PACK),
        Arc::new(AnyFile),
        &key(name),
        &f.path(name),
        &f.ws,
    )
}

/// A `model.ifc` live as text when the routing changes keeps every change: the host demotes
/// it, and the same key is a tier-0 doc with every text version, the last live state current
/// and in the file (or, after an outside save, the save current and the live state a version
/// and a conflict copy), and the change log readable. The old handle is closed, a second
/// demote is a no-op, and later opens stay tier 0.
fn already_live_row(route: &dyn Route, left: Left) -> Result<(), String> {
    const ROW: &str = "a_cad_doc_already_live_as_text_keeps_all_its_content";
    let name = "model.ifc";
    let f = fx("live");
    fs::write(f.path(name), IFC).unwrap();
    // before this change: the file opened as live text of unknown type
    let s = f
        .mgr
        .open_file(Arc::new(PACK), &key(name), &f.path(name), &f.ws)
        .unwrap();
    let a = f.join(&s, "agent-a");
    let with_3 = ifc_plus(IFC, WALL_2, WALL_3);
    write(&s, a, &with_3);
    s.flush(a).unwrap();
    assert_eq!(
        f.bytes(name),
        with_3.as_bytes(),
        "agent a's wall is written"
    );
    let with_4 = ifc_plus(&with_3, &format!("{WALL_3}\n"), DOOR_4);
    let with_window = ifc_plus(&with_3, &format!("{WALL_3}\n"), WINDOW_4);
    // the text doc's handle the demotion closes, and one of its connections
    let (old, old_conn) = match left {
        Left::OpenWithUnwrittenChange | Left::OpenWithOutsideSave => {
            let b = f.join(&s, "agent-b");
            write(&s, b, &with_4);
            assert_eq!(
                f.bytes(name),
                with_3.as_bytes(),
                "b's door isn't written yet"
            );
            if matches!(left, Left::OpenWithOutsideSave) {
                f.host.now.fetch_add(2_000, Ordering::SeqCst);
                let tmp = f.path(".app-save.tmp");
                fs::write(&tmp, &with_window).unwrap();
                fs::rename(&tmp, f.path(name)).unwrap();
            }
            (s, a)
        }
        Left::Closed => {
            let b = f.join(&s, "agent-b");
            write(&s, b, &with_4);
            drop(s);
            assert!(f.mgr.close(&key(name), "shutdown").unwrap());
            assert_eq!(f.bytes(name), with_4.as_bytes(), "the close wrote the door");
            let t = f.mgr.open(Arc::new(PACK), &key(name), None).unwrap();
            let c = f.join(&t, "reader");
            (t, c)
        }
        Left::ClosedWithChangeTheFileNeverGot => {
            drop(s);
            assert!(f.mgr.close(&key(name), "shutdown").unwrap());
            let t = f.mgr.open(Arc::new(PACK), &key(name), None).unwrap();
            let b = f.join(&t, "agent-b");
            write(&t, b, &with_4);
            drop(t);
            assert!(f.mgr.close(&key(name), "crash").unwrap());
            assert_eq!(
                f.bytes(name),
                with_3.as_bytes(),
                "the file never got the door"
            );
            let t = f.mgr.open(Arc::new(PACK), &key(name), None).unwrap();
            let c = f.join(&t, "reader");
            (t, c)
        }
    };
    let before = text_files(&old, old_conn);
    let changes = old
        .log(old_conn, &LogQuery::default())
        .unwrap()
        .into_iter()
        .filter(|i| matches!(i, LogItem::Change { .. }))
        .count() as u64;
    // what the file must hold after the demotion
    let file_after = match left {
        Left::OpenWithOutsideSave => &with_window,
        _ => &with_4,
    };

    // after this change: the next open
    need(route.pack(name).is_none(), ROW, || {
        format!("{left:?}: {name} still opens as live text")
    })?;
    // the kit alone: a tier-0 open of the stored text doc is refused, and nothing changes
    match f
        .mgr
        .open_blob_file(Arc::new(AnyFile), &key(name), &f.path(name), &f.ws)
    {
        Err(SessionError::PackMismatch { stored, pack }) => {
            assert_eq!((stored.as_str(), pack.as_str()), ("text", BLOB_DOC_PACK))
        }
        other => panic!("{left:?}: tier 0 over a stored text doc: {:?}", other.err()),
    }
    // the rule: demote once
    let (blob, d) = demote(&f, name).map_err(|e| format!("{ROW}: {left:?}: {e}"))?;
    need(!d.already && d.key == key(name), ROW, || {
        format!("{left:?}: a first demote: {d:?}")
    })?;
    let c = f.join_blob(&blob, "reader");
    // every text version is a tier-0 version with the bytes its file_at gave
    need(d.versions.len() == before.len(), ROW, || {
        format!(
            "{left:?}: {} text versions became {:?}",
            before.len(),
            d.versions
        )
    })?;
    for ((tv, bytes), (from, to)) in before.iter().zip(&d.versions) {
        need(tv == from, ROW, || {
            format!("{left:?}: version {tv} mapped as {from}")
        })?;
        let got = blob_at(&blob, c, *to);
        need(got == *bytes, ROW, || {
            format!(
                "{left:?}: version {tv} (now {to}) isn't its file_at:\n{}",
                show(&got)
            )
        })?;
    }
    let held: Vec<Vec<u8>> = blob
        .versions(c)
        .unwrap()
        .into_iter()
        .map(|(v, _)| blob_at(&blob, c, v))
        .collect();
    need(held.iter().any(|v| v == IFC.as_bytes()), ROW, || {
        format!("{left:?}: no tier-0 version holds the import")
    })?;
    // the last live state is kept: current, or (after an outside save) a version and a copy
    need(blob_at(&blob, c, d.live) == with_4.as_bytes(), ROW, || {
        format!(
            "{left:?}: the live version isn't the last live state:\n{}",
            show(&blob_at(&blob, c, d.live))
        )
    })?;
    need(f.bytes(name) == file_after.as_bytes(), ROW, || {
        format!("{left:?}: the file is\n{}", show(&f.bytes(name)))
    })?;
    need(blob_current(&blob, c) == file_after.as_bytes(), ROW, || {
        format!(
            "{left:?}: the current version is\n{}",
            show(&blob_current(&blob, c))
        )
    })?;
    need(d.written == left.writes(), ROW, || {
        format!("{left:?}: written is {}: {d:?}", d.written)
    })?;
    match left {
        Left::OpenWithOutsideSave => {
            need(d.kept_both && d.current != d.live, ROW, || {
                format!("{left:?}: not kept both: {d:?}")
            })?;
            let copy = d
                .conflict_copy
                .clone()
                .ok_or(format!("{ROW}: {left:?}: no conflict copy"))?;
            need(
                fs::read(&copy).ok().as_deref() == Some(with_4.as_bytes()),
                ROW,
                || format!("{left:?}: the conflict copy isn't the last live state"),
            )?;
            for t in &held {
                let t = show(t);
                need(
                    !(t.contains("IFCDOOR") && t.contains("IFCWINDOW")),
                    ROW,
                    || format!("{left:?}: a version merges the save by lines:\n{t}"),
                )?;
            }
        }
        _ => need(!d.kept_both && d.current == d.live, ROW, || {
            format!("{left:?}: {d:?}")
        })?,
    }
    // the change log stays readable, every change by its author
    let authors: Vec<String> = blob
        .live_log(c)
        .unwrap()
        .into_iter()
        .filter_map(|i| match i {
            LogItem::Change { info, .. } => Some(info.author),
            _ => None,
        })
        .collect();
    need(
        d.changes == changes && authors.len() as u64 == changes,
        ROW,
        || {
            format!(
                "{left:?}: {changes} changes, {} kept, live log {authors:?}",
                d.changes
            )
        },
    )?;
    for pid in ["agent-a", "agent-b"] {
        need(authors.iter().any(|a| a == pid), ROW, || {
            format!("{left:?}: the live log has no change by {pid}: {authors:?}")
        })?;
    }
    // the old handle is closed
    need(
        matches!(old.read(old_conn), Err(SessionError::Closed { reason }) if reason == DEMOTED),
        ROW,
        || format!("{left:?}: the text session isn't closed as demoted"),
    )?;
    // a second demote is a no-op
    let (again, d2) = demote(&f, name).map_err(|e| format!("{ROW}: {left:?}: again: {e}"))?;
    need(d2.already && d2.versions.is_empty(), ROW, || {
        format!("{left:?}: a second demote: {d2:?}")
    })?;
    need(Arc::ptr_eq(&again, &blob), ROW, || {
        format!("{left:?}: a second demote isn't the tier-0 doc")
    })?;
    // later opens stay tier 0: the same blob doc
    let again = f
        .mgr
        .open_blob_file(Arc::new(AnyFile), &key(name), &f.path(name), &f.ws)
        .map_err(|e| format!("{ROW}: {left:?}: tier-0 open: {e}"))?;
    need(Arc::ptr_eq(&again, &blob), ROW, || {
        format!("{left:?}: the next open isn't the tier-0 doc")
    })?;
    need(
        blob_current(&again, c) == file_after.as_bytes(),
        ROW,
        || format!("{left:?}: the tier-0 open moved the current version"),
    )?;
    match f
        .mgr
        .open_file(Arc::new(PACK), &key(name), &f.path(name), &f.ws)
    {
        Err(SessionError::PackMismatch { stored, .. }) if stored == BLOB_DOC_PACK => Ok(()),
        other => Err(format!(
            "{ROW}: {left:?}: a text open after the demote: {:?}",
            other.err()
        )),
    }
}

const LEFT: [Left; 4] = [
    Left::OpenWithUnwrittenChange,
    Left::Closed,
    Left::ClosedWithChangeTheFileNeverGot,
    Left::OpenWithOutsideSave,
];

#[test]
fn a_cad_doc_already_live_as_text_keeps_all_its_content() {
    for left in LEFT {
        already_live_row(&Table, left).unwrap();
    }
}

/// The plant: with the routing before this change, each CAD row goes red, by name.
#[test]
fn the_planted_cad_extension_live_is_caught_by_name() {
    let e = cad_row(&CadExtensionLive).expect_err("planted, the extension row fails");
    assert!(e.starts_with("each_cad_extension_is_never_text: "), "{e}");
    for left in LEFT {
        let e = already_live_row(&CadExtensionLive, left)
            .expect_err("planted, the already-live row fails");
        assert!(
            e.starts_with("a_cad_doc_already_live_as_text_keeps_all_its_content: "),
            "{e}"
        );
        assert!(e.contains("still opens as live text"), "{e}");
    }
}
