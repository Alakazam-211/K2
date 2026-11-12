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
//! A doc already live as text when the routing changes keeps all its content: the host closes
//! it (a pending write-back is written first), moves the text doc with its whole history to a
//! key no file has (`SessionManager::rekey`), and opens the file at tier 0, whose first version
//! is the file as the text doc last wrote it. The kit has no text-to-tier-0 path of its own
//! (`open_blob_file` on a stored text doc is `PackMismatch`, nothing changed), so the row runs
//! that rule with the kit's calls, as a host would.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, BlobSession, Config, ConnId, Host, LogItem, LogQuery, Pack, Participant,
    ParticipantKind, Rights, Session, SessionError, SessionManager, Stay, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore, BLOB_DOC_PACK};
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

/// Where a retired text doc's history goes: a space no workspace file is in (the host's
/// choice; a key no file can have).
fn history_key(p: &str) -> DocKey {
    DocKey {
        space: "retired-text".into(),
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

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root.remove(0).1
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
}

/// The host's rule for a text doc whose name the pack now calls not-text: close it (a
/// pending write-back is written first), move it with its whole history to `history`, then
/// open the file at tier 0 (imported from the file as it now is: its first version).
fn retire_to_tier0(f: &Fx, name: &str, history: &DocKey) -> Result<Arc<BlobSession>, SessionError> {
    f.mgr.close(&key(name), "not_text")?;
    f.mgr.rekey(&key(name), history.clone())?;
    f.mgr
        .open_blob_file(Arc::new(AnyFile), &key(name), &f.path(name), &f.ws)
}

/// A `model.ifc` live as text when the routing changes keeps every change: the file and the
/// tier-0 doc's first version hold the last state the file was written with, the moved text
/// doc holds the last live state and its whole history, and the next open stays tier 0.
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
    // what the file must hold once the doc is retired, and what the text doc last held
    let (file_after, live_last) = match left {
        Left::OpenWithUnwrittenChange => {
            let b = f.join(&s, "agent-b");
            write(&s, b, &with_4);
            assert_eq!(
                f.bytes(name),
                with_3.as_bytes(),
                "b's door isn't written yet"
            );
            (with_4.clone(), with_4.clone())
        }
        Left::Closed => {
            let b = f.join(&s, "agent-b");
            write(&s, b, &with_4);
            drop(s);
            assert!(f.mgr.close(&key(name), "shutdown").unwrap());
            assert_eq!(f.bytes(name), with_4.as_bytes(), "the close wrote the door");
            (with_4.clone(), with_4.clone())
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
            (with_3.clone(), with_4.clone())
        }
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
    // the rule
    let history = history_key(name);
    let blob = retire_to_tier0(&f, name, &history).map_err(|e| format!("{ROW}: {left:?}: {e}"))?;
    need(f.bytes(name) == file_after.as_bytes(), ROW, || {
        format!(
            "{left:?}: the file isn't the last written state:\n{}",
            String::from_utf8_lossy(&f.bytes(name))
        )
    })?;
    let c = f.join_blob(&blob, "reader");
    let mut cur = Vec::new();
    blob.read(c).unwrap().reader.read_to_end(&mut cur).unwrap();
    need(cur == file_after.as_bytes(), ROW, || {
        format!("{left:?}: the tier-0 doc isn't the file")
    })?;
    let n = blob.versions(c).unwrap().len();
    need(n == 1, ROW, || {
        format!("{left:?}: the tier-0 doc has {n} versions")
    })?;
    // the text doc, moved, holds the last live state and its history
    let h = f.mgr.open(Arc::new(PACK), &history, None).unwrap();
    let hc = f.join(&h, "reader");
    let last = text(&h, hc);
    need(last == live_last, ROW, || {
        format!("{left:?}: the moved text doc isn't the last live state:\n{last}")
    })?;
    let shown: Vec<String> = h
        .versions(hc)
        .unwrap()
        .iter()
        .map(|v| h.show(hc, v.id).unwrap()[0].1.clone())
        .collect();
    need(shown.iter().any(|v| v == IFC), ROW, || {
        format!("{left:?}: no version of the moved text doc holds the import")
    })?;
    // every change, by its author (versions are cut by time, not per change)
    let authors: Vec<String> = h
        .log(hc, &LogQuery::default())
        .unwrap()
        .into_iter()
        .filter_map(|i| match i {
            LogItem::Change { info, .. } => Some(info.author),
            _ => None,
        })
        .collect();
    for pid in ["agent-a", "agent-b"] {
        need(authors.iter().any(|a| a == pid), ROW, || {
            format!("{left:?}: the moved text doc's log has no change by {pid}: {authors:?}")
        })?;
    }
    // the next open stays tier 0: the same blob doc
    let again = f
        .mgr
        .open_blob_file(Arc::new(AnyFile), &key(name), &f.path(name), &f.ws)
        .unwrap();
    need(Arc::ptr_eq(&again, &blob), ROW, || {
        format!("{left:?}: the next open isn't the tier-0 doc")
    })?;
    match f
        .mgr
        .open_file(Arc::new(PACK), &key(name), &f.path(name), &f.ws)
    {
        Err(SessionError::PackMismatch { stored, .. }) if stored == BLOB_DOC_PACK => Ok(()),
        other => Err(format!(
            "{ROW}: {left:?}: a text open after the rule: {:?}",
            other.err()
        )),
    }
}

const LEFT: [Left; 3] = [
    Left::OpenWithUnwrittenChange,
    Left::Closed,
    Left::ClosedWithChangeTheFileNeverGot,
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
