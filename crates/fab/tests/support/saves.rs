//! The photoplot pack through outside saves: a fresh export written over the file, as an
//! exporter does, taken in by a kit blob session that keeps the file in sync
//! (`file_changed` after each write on disk), each version indexed. Four rows:
//!
//! 1. **no-change save**: the same bytes written again. No new version; blame unmoved.
//! 2. **churn save**: every aperture renumbered. Every key kept; the diff only renames each
//!    aperture and its objects and changes their number attributes; no node added, removed,
//!    moved, or changed in record, content or text.
//! 3. **reuse save**: two apertures swap numbers (D11 becomes the obround, D12 the rectangle).
//!    Each definition keeps its key and its objects; the diff only renames and renumbers. The
//!    plant `KeyByNumber` (keys by number, which the save reissues) must fail this row.
//! 4. **a kit write between two outside saves**: a person moves a pad through the kit, then an
//!    outside save moves another. Blame credits the person's version for the first pad's group
//!    and the outside save for the other's.
//!
//! Each row returns `Err` naming the step that failed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::index::{IndexStatus, NodeChange, NodeChangeKind};
use k2_mpc_base::session::{
    AuditEvent, BlobSession, Config, ConnId, DiskCheck, Host, Participant, ParticipantKind, Rights,
    SessionManager, Stay, EXTERNAL_PID,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionId};
use k2_mpc_fab::PhotoplotPack;

use super::BOARD;

struct TestHost {
    rights: Mutex<HashMap<String, Rights>>,
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
        1_000_000
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

/// A file-backed session of the board, a person with history, and the file's path.
struct Fx {
    dir: PathBuf,
    file: PathBuf,
    host: Arc<TestHost>,
    _mgr: SessionManager,
    s: Arc<BlobSession>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str, first: &str) -> Result<Fx, String> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2fab-saves-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).map_err(|e| format!("{tag}: making the folder: {e}"))?;
    let ws = std::fs::canonicalize(&ws).map_err(|e| format!("{tag}: {e}"))?;
    let file = ws.join("top.gbr");
    std::fs::write(&file, first).map_err(|e| format!("{tag}: writing the file: {e}"))?;
    let host = Arc::new(TestHost {
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).map_err(|e| format!("{e:?}"))?);
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(|| 1_000_000),
        )
        .map_err(|e| format!("{e:?}"))?,
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    let key = DocKey {
        space: "ws".into(),
        path: "top.gbr".into(),
    };
    let s = mgr
        .open_blob_file(Arc::new(PhotoplotPack), &key, &file, &ws)
        .map_err(|e| format!("{tag}: the file opens: {e:?}"))?;
    let f = Fx {
        dir,
        file,
        host,
        _mgr: mgr,
        s,
    };
    f.index("the first version")?;
    Ok(f)
}

impl Fx {
    fn person(&self, pid: &str) -> Result<ConnId, String> {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::editor().with_history(true));
        self.s
            .join(
                Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab"),
                Stay::Held,
            )
            .map(|j| j.conn)
            .map_err(|e| format!("{pid} joins: {e:?}"))
    }

    fn version(&self) -> Result<VersionId, String> {
        self.s
            .status()
            .map(|st| st.version)
            .map_err(|e| format!("status: {e:?}"))
    }

    /// Runs the current version's index job, if one is due, and checks it is ready.
    fn index(&self, step: &str) -> Result<(), String> {
        if let Some(job) = self
            .s
            .index_job(None)
            .map_err(|e| format!("{step}: {e:?}"))?
        {
            let st = job.run().map_err(|e| format!("{step}: the job: {e:?}"))?;
            if !matches!(st, IndexStatus::Ready { .. }) {
                return Err(format!("{step}: indexed as {st:?}"));
            }
        }
        Ok(())
    }

    /// An outside save: `bytes` written over the file, then the kit told; its new version
    /// indexed. The check the kit made, for the row to assert.
    fn outside_save(&self, step: &str, bytes: &str) -> Result<DiskCheck, String> {
        write_file(&self.file, bytes, step)?;
        let check = self
            .s
            .file_changed()
            .map_err(|e| format!("{step}: file_changed: {e:?}"))?;
        self.index(step)?;
        Ok(check)
    }
}

fn write_file(path: &Path, bytes: &str, step: &str) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{step}: writing the file: {e}"))
}

/// The board with every aperture renumbered (D10 to D15 as D50 to D55, D20 and D21 as D60 and
/// D61): the same definitions and objects under other numbers, as a fresh export may number
/// them. `D01`, `D02` and `D03` are operations, not apertures, and keep their codes.
pub fn renumbered(board: &str) -> String {
    board.replace("D1", "D5").replace("D2", "D6")
}

/// The board with D11 and D12 swapped: the rectangle is D12 and the obround D11, each with its
/// own objects.
pub fn swapped(board: &str) -> String {
    board
        .replace("D11", "D\u{0}")
        .replace("D12", "D11")
        .replace("D\u{0}", "D12")
}

/// The changes of `from` to `to`, by key, as the person reads them.
fn diff(f: &Fx, c: ConnId, from: VersionId, to: VersionId) -> Result<Vec<NodeChange>, String> {
    f.s.index_diff(c, from, to)
        .map(|d| d.changes)
        .map_err(|e| format!("diff {from} to {to}: {e:?}"))
}

/// Every change is a rename and a change of the `number` or `aperture` attribute, nothing
/// else; at least `renamed` nodes renamed.
fn only_renumbered(changes: &[NodeChange], renamed: usize, step: &str) -> Result<(), String> {
    let allowed: BTreeSet<NodeChangeKind> = [NodeChangeKind::Renamed, NodeChangeKind::Props]
        .into_iter()
        .collect();
    for c in changes {
        let other: Vec<&str> = c
            .changes
            .iter()
            .filter(|k| !allowed.contains(k))
            .map(|k| k.as_str())
            .collect();
        if !other.is_empty() {
            return Err(format!(
                "{step}: {} changed as {other:?} (props {:?}); a renumbering changes names and numbers only",
                c.key, c.props
            ));
        }
        if let Some(p) = c
            .props
            .iter()
            .find(|p| *p != "attrs.number" && *p != "attrs.aperture")
        {
            return Err(format!("{step}: {} changed {p}", c.key));
        }
    }
    let n = changes
        .iter()
        .filter(|c| c.changes.contains(&NodeChangeKind::Renamed))
        .count();
    if n < renamed {
        return Err(format!(
            "{step}: {n} nodes renamed, at least {renamed} expected (the renumbered ones)"
        ));
    }
    Ok(())
}

fn keys_of(f: &Fx, step: &str) -> Result<BTreeSet<String>, String> {
    let v =
        f.s.index_view(None)
            .map_err(|e| format!("{step}: the view: {e:?}"))?;
    Ok(v.nodes.iter().map(|n| n.key.as_str().to_owned()).collect())
}

/// Row 1: the same bytes written again from outside: no new version, blame unmoved.
pub fn row_no_change_save() -> Result<(), String> {
    let f = fx("same", BOARD)?;
    let p = f.person("ada")?;
    let v1 = f.version()?;
    let before =
        f.s.index_blame(p, None)
            .map_err(|e| format!("blame before: {e:?}"))?;
    let check = f.outside_save("the same bytes", BOARD)?;
    if !matches!(check, DiskCheck::Unchanged) {
        return Err(format!(
            "the same bytes: the kit saw {check:?}, not unchanged"
        ));
    }
    if f.version()? != v1 {
        return Err("the same bytes made a version".into());
    }
    let after =
        f.s.index_blame(p, None)
            .map_err(|e| format!("blame after: {e:?}"))?;
    if before.rows != after.rows {
        return Err("the same bytes moved blame".into());
    }
    println!(
        "row no-change save: {} nodes, version {v1} kept",
        after.rows.len()
    );
    Ok(())
}

/// Row 2: every aperture renumbered: every key kept, the diff renames and renumbers only.
pub fn row_churn_save_keeps_every_key() -> Result<(), String> {
    let f = fx("churn", BOARD)?;
    let p = f.person("ada")?;
    let v1 = f.version()?;
    let keys1 = keys_of(&f, "before")?;
    let check = f.outside_save("renumbered", &renumbered(BOARD))?;
    if !matches!(check, DiskCheck::Ingested { .. }) {
        return Err(format!(
            "renumbered: the kit saw {check:?}, not an outside save"
        ));
    }
    let v2 = f.version()?;
    let keys2 = keys_of(&f, "after")?;
    if keys1 != keys2 {
        let gone: Vec<_> = keys1.difference(&keys2).collect();
        let new: Vec<_> = keys2.difference(&keys1).collect();
        return Err(format!(
            "renumbered: keys not kept: gone {gone:?}, new {new:?}"
        ));
    }
    let changes = diff(&f, p, v1, v2)?;
    // eight apertures and the six groups of objects made with the plotting ones
    only_renumbered(&changes, 8, "renumbered")?;
    println!(
        "row churn save: {} keys kept, {} nodes renamed or renumbered",
        keys2.len(),
        changes.len()
    );
    Ok(())
}

/// Row 3: two apertures swap numbers: each definition keeps its key and its objects. The plant
/// `KeyByNumber` fails it.
pub fn row_reuse_save_keeps_each_definition() -> Result<(), String> {
    let f = fx("reuse", BOARD)?;
    let p = f.person("ada")?;
    let v1 = f.version()?;
    let check = f.outside_save("swapped", &swapped(BOARD))?;
    if !matches!(check, DiskCheck::Ingested { .. }) {
        return Err(format!(
            "swapped: the kit saw {check:?}, not an outside save"
        ));
    }
    let v2 = f.version()?;
    let changes = diff(&f, p, v1, v2)?;
    // D11 and D12 renamed, and their two groups of objects
    only_renumbered(&changes, 4, "swapped")?;
    println!(
        "row reuse save: {} nodes renamed or renumbered, none changed",
        changes.len()
    );
    Ok(())
}

/// Row 4: a person's write through the kit, then an outside save that changes another group:
/// blame credits each version with what it changed.
pub fn row_kit_write_between_outside_saves() -> Result<(), String> {
    let f = fx("between", BOARD)?;
    let p = f.person("ada")?;
    let v1 = f.version()?;
    // the person moves the obround pad (D12) through the kit
    let mine = BOARD.replace("X8000000Y8000000D03*", "X8100000Y8000000D03*");
    if mine == BOARD {
        return Err("the person's edit changed nothing".into());
    }
    f.s.write_bytes(p, mine.as_bytes(), v1)
        .map_err(|e| format!("the person's write: {e:?}"))?;
    let v2 = f.version()?;
    f.index("the person's version")?;
    // then an outside save moves the polygon pad (D13), keeping the person's move
    let theirs = mine.replace("X1000000Y9000000D03*", "X1100000Y9000000D03*");
    let check = f.outside_save("the outside save", &theirs)?;
    if !matches!(
        check,
        DiskCheck::Ingested { .. } | DiskCheck::Conflict { .. }
    ) {
        return Err(format!(
            "the outside save: the kit saw {check:?}, not an outside save"
        ));
    }
    let v3 = f.version()?;
    let blame =
        f.s.index_blame(p, None)
            .map_err(|e| format!("blame: {e:?}"))?;
    let rows: BTreeMap<&str, (VersionId, &str)> = blame
        .rows
        .iter()
        .map(|r| (r.key.as_str(), (r.version, r.author.as_str())))
        .collect();
    let obround = "@objects:obround,1.2,0.8;SMDPad,CuDef";
    let polygon_prefix = "@objects:polygon,";
    match rows.get(obround) {
        Some((v, who)) if *v == v2 && *who == "ada" => {}
        other => {
            return Err(format!(
                "blame of {obround}: {other:?}; the person's version {v2} by ada expected"
            ))
        }
    }
    let polygon = rows
        .iter()
        .find(|(k, _)| k.starts_with(polygon_prefix))
        .ok_or("no polygon group in the blame")?;
    if polygon.1 .0 != v3 || polygon.1 .1 != EXTERNAL_PID {
        return Err(format!(
            "blame of {}: {:?}; the outside save {v3} expected",
            polygon.0, polygon.1
        ));
    }
    println!("row kit write between outside saves: the person's {v2}, the save's {v3}");
    Ok(())
}
