//! N−1 of the text → tier 0 demotion (tools/n1/n1.sh `layout-demote`), built against the
//! previous release, which has no demotion and doesn't know its table (`demoted_changes`).
//!
//! - `read <dir>`: `<dir>/fwd` is a store this build's `n1_demote_producer` wrote, with two
//!   demoted docs. The previous release opens each at tier 0 and reads every version, byte for
//!   byte, and the current one as `expect.txt` lists them (never a misreading: a demoted doc is
//!   a plain tier-0 doc to it, without its live log); a text open is refused `PackMismatch`
//!   (`stored: "kit:blob"`); a purge and an erasure are refused before anything changes
//!   (`NewerData`, the table named) by a release before v0.17.0, and done by v0.17.0 and later,
//!   which know the table (`K2MB_N1_PREV`, the previous release's crate version, which `n1.sh`
//!   reads from its manifest); it writes a version to `model.ifc` and deletes `gone.ifc` (its
//!   live-log rows left for this repo's next open to sweep, or taken with it by a release that
//!   knows them).
//! - `produce <dir>`: it writes `<dir>/rev`, a text doc `model.ifc` with a change the file
//!   never got (made without the file, as when a process stops before its write-back) and then
//!   an outside save, for this repo's `n1_demote_prev_store` to demote. `live.txt` holds its
//!   live state.
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, ConnId, Host, Participant, ParticipantKind, Rights, Session,
    SessionError, SessionManager, Stay, TextFilePack, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError, BLOB_DOC_PACK};

const TXT: TextFilePack = TextFilePack::new("text", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;

struct H {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for H {
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
    fn on_event(&self, _ev: &AuditEvent) {}
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn open(dir: &Path) -> (Arc<H>, SessionManager, PathBuf) {
    let host = Arc::new(H {
        now: AtomicU64::new(1_793_200_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    {
        let mut r = host.rights.lock().unwrap();
        r.insert("own".into(), Rights::full().with_owner(true));
        r.insert("agent-a".into(), Rights::editor());
    }
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
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).unwrap();
    (host, mgr, fs::canonicalize(ws).unwrap())
}

fn person(pid: &str) -> Participant {
    Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab")
}

/// Whether this build knows the demotion's table (`demoted_changes`): its crate version,
/// `K2MB_N1_PREV`, is v0.17.0 or later. Unset: a build before it.
fn knows_demoted() -> bool {
    let Ok(v) = std::env::var("K2MB_N1_PREV") else {
        return false;
    };
    let n: Vec<u64> = v.trim().split('.').map(|x| x.parse().unwrap()).collect();
    assert_eq!(n.len(), 3, "K2MB_N1_PREV={v:?}: not MAJOR.MINOR.PATCH");
    (n[0], n[1], n[2]) >= (0, 17, 0)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn read(dir: &Path) {
    let dir = dir.join("fwd");
    let expect = fs::read_to_string(dir.join("expect.txt")).unwrap();
    let (_h, mgr, ws) = open(&dir);
    for name in ["model.ifc", "gone.ifc"] {
        let b = mgr
            .open_blob_file(Arc::new(AnyFile), &key(name), &ws.join(name), &ws)
            .unwrap_or_else(|e| panic!("{name}: a tier-0 open: {e}"));
        let c = b.join(person("own"), Stay::Held).unwrap().conn;
        let mut n = 0;
        for line in expect
            .lines()
            .filter(|l| l.starts_with(&format!("{name} ")))
        {
            let mut p = line.split(' ');
            let (_, v, want) = (p.next(), p.next().unwrap(), p.next().unwrap());
            if v == "current" {
                assert_eq!(
                    b.status().unwrap().version.to_string(),
                    want,
                    "{name}: the current version"
                );
                continue;
            }
            let mut got = Vec::new();
            b.read_version(c, v.parse().unwrap())
                .unwrap()
                .reader
                .read_to_end(&mut got)
                .unwrap();
            assert_eq!(hex(&got), want, "{name}: version {v}'s bytes");
            n += 1;
        }
        assert!(n > 0, "{name}: expect.txt lists no version");
        assert_eq!(
            b.versions(c).unwrap().len(),
            n,
            "{name}: the versions this build lists"
        );
        println!("{name}: {n} versions read exactly");
        match mgr.open_file(Arc::new(TXT), &key(name), &ws.join(name), &ws) {
            Err(SessionError::PackMismatch { stored, .. }) => {
                assert_eq!(stored, BLOB_DOC_PACK);
                println!("{name}: text open refused (PackMismatch, stored {stored})");
            }
            other => panic!("{name}: a text open: {:?}", other.map(|_| ())),
        }
        if name == "model.ifc" {
            // a release that knows `demoted_changes` purges and erases (the erasure renames
            // agent-a in the live log: the current build's `n1_demote_after` checks it); an
            // older one refuses both before anything changes
            let knows = knows_demoted();
            let outcome = |r: Result<(), SessionError>, what: &str| match r {
                Ok(()) if knows => println!("{name}: {what} ok (this release knows the table)"),
                Err(SessionError::Store(StoreError::NewerData { table })) if !knows => {
                    assert_eq!(table, "demoted_changes", "{what}");
                    println!("{name}: {what} refused (NewerData {table})");
                }
                other => panic!("{name}: {what} (knows demoted_changes: {knows}): {other:?}"),
            };
            outcome(b.purge(c, None).map(|_| ()), "purge");
            outcome(
                b.erase_participant(c, "agent-a", "erased-1").map(|_| ()),
                "erasure",
            );
            let base = b.status().unwrap().version;
            b.write_bytes(c, b"written by the previous release\n", base)
                .unwrap();
            println!("{name}: a version written");
        }
        drop(b);
    }
    mgr.delete_doc(&key("gone.ifc")).unwrap();
    println!("gone.ifc: deleted");
    mgr.release().unwrap();
    println!("N-1 DEMOTE READ OK");
}

const IFC: &str = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n\
#1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'Project',$,$,$,$,$,$);\n\
#2=IFCWALL('2O2Fr$t4X7Zf8NOew3FLOH',$,'Wall A',$,$,$,$,$,$);\n\
ENDSEC;\nEND-ISO-10303-21;\n";
const WALL_2: &str = "#2=IFCWALL('2O2Fr$t4X7Zf8NOew3FLOH',$,'Wall A',$,$,$,$,$,$);\n";
const WALL_3: &str = "#3=IFCWALL('1hOSvn6df7F8_7GcBWlRGQ',$,'Wall B',$,$,$,$,$,$);\n";
const DOOR_4: &str = "#4=IFCDOOR('3cUkl32yn9qRSPvBJVyWYp',$,'Door 1',$,$,$,$,$,$);\n";
const WINDOW_4: &str = "#4=IFCWINDOW('0dWcU0d4LBqhgsE5zUPF0X',$,'Window 1',$,$,$,$,$,$);\n";

fn plus(base: &str, after: &str, line: &str) -> String {
    let i = base.find(after).unwrap() + after.len();
    format!("{}{line}{}", &base[..i], &base[i..])
}

fn write(s: &Session, c: ConnId, to: &str) {
    let v = s.read(c).unwrap().version;
    let out = s.write_base(c, ROOT, v, to).unwrap();
    assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
}

fn produce(dir: &Path) {
    let dir = dir.join("rev");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let (h, mgr, ws) = open(&dir);
    let name = "model.ifc";
    let path = ws.join(name);
    fs::write(&path, IFC).unwrap();
    let with_3 = plus(IFC, WALL_2, WALL_3);
    let with_door = plus(&with_3, WALL_3, DOOR_4);
    let s = mgr
        .open_file(Arc::new(TXT), &key(name), &path, &ws)
        .unwrap();
    let a = s
        .join(
            Participant::new(
                "agent-a",
                "agent-a",
                "#654321",
                ParticipantKind::Agent,
                "cli",
            ),
            Stay::Held,
        )
        .unwrap()
        .conn;
    write(&s, a, &with_3);
    s.flush(a).unwrap();
    drop(s);
    assert!(mgr.close(&key(name), "shutdown").unwrap());
    // a change the file never got
    let t = mgr.open(Arc::new(TXT), &key(name), None).unwrap();
    let a = t
        .join(
            Participant::new(
                "agent-a",
                "agent-a",
                "#654321",
                ParticipantKind::Agent,
                "cli",
            ),
            Stay::Held,
        )
        .unwrap()
        .conn;
    write(&t, a, &with_door);
    drop(t);
    assert!(mgr.close(&key(name), "stopped").unwrap());
    assert_eq!(fs::read(&path).unwrap(), with_3.as_bytes());
    mgr.release().unwrap();
    h.now.fetch_add(5_000, Ordering::SeqCst);
    // then another program saves the file
    fs::write(&path, plus(&with_3, WALL_3, WINDOW_4)).unwrap();
    fs::write(dir.join("live.txt"), &with_door).unwrap();
    println!("N-1 DEMOTE PRODUCED (previous release)");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("read <dir> | produce <dir>");
    let dir = PathBuf::from(args.next().expect("dir"));
    match mode.as_str() {
        "read" => read(&dir),
        "produce" => produce(&dir),
        m => panic!("unknown mode {m}"),
    }
}
