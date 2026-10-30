//! Tier 0 (any file, no type pack) as a host uses it, on real temp folders with the host's
//! injected clock: presence on a PDF, the lease, `write --base`, a desktop app saving under
//! someone's lease (conflict copy, nothing lost), restore, named versions, the blob store's
//! keyed addresses, verification, GC and purge.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobError, BlobStore};
use k2_mpc_base::session::{
    Action, AnyFile, Approval, ApprovalState, AuditEvent, BlobLogEntry, BlobLogKind, BlobMerge,
    BlobPack, BlobSession, Config, ConnId, DiskCheck, DiskState, Host, LeaseEnd, Need, Participant,
    ParticipantKind, Rights, Role, SessionError, SessionManager, Stay, TextFilePack, EXTERNAL_PID,
};
use k2_mpc_base::store::{BlobReason, DocKey, DocStore, StoreError};

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    events: Mutex<Vec<AuditEvent>>,
    /// Where conflict copies go (`Host::conflict_dir`): (the folder, the workspace it
    /// mirrors); `None` = beside the file.
    conflicts: Mutex<Option<(PathBuf, PathBuf)>>,
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
    fn conflict_dir(&self, file: &Path) -> Option<PathBuf> {
        // mirror the file's folder under the host's conflicts folder
        let (root, ws) = self.conflicts.lock().unwrap().clone()?;
        let rel = file.parent()?.strip_prefix(&ws).ok()?;
        Some(root.join(rel))
    }
}

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.0);
    }
}

use std::os::unix::fs::PermissionsExt;

struct Fx {
    dir: TmpDir,
    ws: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    blobs: Arc<BlobStore>,
    mgr: SessionManager,
}

// 2026-10-27 14:30:00 UTC
const T0: u64 = 1_793_111_400_000;

fn fx_with(tag: &str, bc: BlobConfig) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-t0-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(T0),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
        conflicts: Mutex::new(None),
    });
    let store = Arc::new(DocStore::open(p.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            p.join("blobs"),
            bc,
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs.clone());
    Fx {
        dir: TmpDir(p),
        ws,
        host,
        store,
        blobs,
        mgr,
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, BlobConfig::default())
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn person(pid: &str, name: &str) -> Participant {
    Participant::new(pid, name, "#123456", ParticipantKind::Person, "tab")
}

fn agent(pid: &str) -> Participant {
    Participant::new(pid, pid, "#654321", ParticipantKind::Agent, "cli")
}

impl Fx {
    fn path(&self, name: &str) -> PathBuf {
        self.ws.join(name)
    }
    fn grant(&self, pid: &str, r: Rights) {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn now(&self) -> u64 {
        self.host.now.load(Ordering::SeqCst)
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
    /// The host keeps conflict copies under `<ws>/conflicts/` (`None`: beside the file).
    fn conflicts_in(&self, folder: Option<&str>) -> Option<PathBuf> {
        let dir = folder.map(|f| self.ws.join(f));
        *self.host.conflicts.lock().unwrap() = dir.clone().map(|d| (d, self.ws.clone()));
        dir
    }
    fn open(&self, name: &str) -> Arc<BlobSession> {
        self.mgr
            .open_blob_file(Arc::new(AnyFile), &key(name), &self.path(name), &self.ws)
            .unwrap()
    }
    fn join(&self, s: &BlobSession, p: Participant) -> ConnId {
        s.join(p, Stay::Held).unwrap().conn
    }
}

fn read_all(s: &BlobSession, c: ConnId) -> Vec<u8> {
    let mut r = s.read(c).unwrap();
    let mut out = Vec::new();
    r.reader.read_to_end(&mut out).unwrap();
    out
}

/// A desktop app's save: write a temp file beside the original and rename it over.
fn app_save(path: &Path, bytes: &[u8]) {
    let tmp = path.with_file_name(".app-save.tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

const PDF1: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n";
const PDF2: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Lang (en) >> endobj\n%%EOF\n";
const PDF3: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Title (desktop) >> endobj\n%%EOF\n";

// ---------- presence ----------

#[test]
fn two_people_view_a_pdf() {
    let f = fx("view");
    fs::write(f.path("paper.pdf"), PDF1).unwrap();
    f.grant("alice", Rights::editor());
    f.grant("bob", Rights::read_only());
    let s = f.open("paper.pdf");
    let a = f.join(&s, person("alice", "Alice"));

    // alone: `who` lists only you, and there is nothing else to show
    let r = s.roster(a).unwrap();
    assert_eq!(r.len(), 1, "alone, the roster is just me");
    assert_eq!(r[0].participant.pid, "alice");
    assert!(!r[0].holds_lease && r[0].region.is_none() && r[0].cursor.is_none());
    assert_eq!(s.status().unwrap().lease, None);

    let b = f.join(&s, person("bob", "Bob"));
    s.set_region(b, Some("page 3".into())).unwrap();
    s.set_region(a, Some("page 1".into())).unwrap();
    let r = s.roster(b).unwrap();
    assert_eq!(r.len(), 2);
    assert!(
        r.iter().all(|e| e.role == Role::Viewer),
        "nobody is editing"
    );
    let bob = r.iter().find(|e| e.participant.pid == "bob").unwrap();
    assert_eq!(bob.region.as_deref(), Some("page 3"));
    assert_eq!(s.status().unwrap().participants, 2);

    // a second tab is one roster entry with two connections
    let b2 = f.join(&s, person("bob", "Bob"));
    let r = s.roster(a).unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(
        r.iter()
            .find(|e| e.participant.pid == "bob")
            .unwrap()
            .connections,
        2
    );

    // away once every connection is hidden for a minute
    s.set_visible(b, false).unwrap();
    s.set_visible(b2, false).unwrap();
    f.advance(61_000);
    let r = s.roster(a).unwrap();
    assert_eq!(r.last().unwrap().participant.pid, "bob");
    assert!(r.last().unwrap().away);

    // regions are bounded; reading needs read rights
    assert!(matches!(
        s.set_region(a, Some("x".repeat(300))),
        Err(SessionError::BadPosition(_))
    ));
    assert_eq!(read_all(&s, b), PDF1, "a viewer reads the file");

    s.leave(b).unwrap();
    s.leave(b2).unwrap();
    assert_eq!(s.roster(a).unwrap().len(), 1, "alone again");
}

// ---------- the lease ----------

#[test]
fn lease_contention_expiry_renew_and_owner_break() {
    let f = fx("lease");
    fs::write(f.path("deck.key"), PDF1).unwrap();
    f.grant("alice", Rights::editor());
    f.grant("agent", Rights::editor());
    f.grant("owner", Rights::full().with_owner(true));
    f.grant("editor2", Rights::full());
    let s = f.open("deck.key");
    let a = f.join(&s, person("alice", "Alice"));
    let g = f.join(&s, agent("agent"));

    // nobody holds it: renew is NoLease
    assert_eq!(s.renew_lease(a, None), Err(SessionError::NoLease));

    let l = s.acquire_lease(a, None).unwrap();
    assert_eq!(l.pid, "alice");
    assert_eq!(l.expires_ms, f.now() + 5 * 60_000, "default 5 min (K-T0-2)");
    // shown in presence: the holder is an editor even without recent edits
    f.advance(45_000);
    let r = s.roster(g).unwrap();
    assert_eq!(r[0].participant.pid, "alice");
    assert!(r[0].holds_lease);
    assert_eq!(r[0].role, Role::Editor);

    // the agent is blocked, with a typed error naming the holder and expiry (exit 5)
    let want = SessionError::Leased {
        holder: "alice".into(),
        name: "Alice".into(),
        expires_ms: l.expires_ms,
    };
    assert_eq!(s.acquire_lease(g, None), Err(want.clone()));
    assert_eq!(s.renew_lease(g, None), Err(want.clone()));
    let v = s.status().unwrap().version;
    assert_eq!(s.write_bytes(g, PDF2, v).unwrap_err(), want);
    assert_eq!(
        fs::read(f.path("deck.key")).unwrap(),
        PDF1,
        "nothing written"
    );
    // leases never block reads
    assert_eq!(read_all(&s, g), PDF1);
    // releasing someone else's lease does nothing
    assert!(!s.release_lease(g).unwrap());

    // renew extends; the holder may ask for a ttl (capped at lease_max_ms)
    let l2 = s.renew_lease(a, Some(10 * 60_000)).unwrap();
    assert_eq!(l2.expires_ms, f.now() + 10 * 60_000);
    assert_eq!(l2.acquired_ms, l.acquired_ms, "a renewal keeps the start");
    let l3 = s.renew_lease(a, Some(24 * 3_600_000)).unwrap();
    assert_eq!(l3.expires_ms, f.now() + 3_600_000, "capped at 1 h");

    // expiry on the host's clock
    f.advance(3_600_000);
    assert_eq!(s.lease().unwrap(), None);
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::LeaseEnded { pid, how: LeaseEnd::Expired, .. } if pid == "alice"
    )));
    let lg = s.acquire_lease(g, Some(60_000)).unwrap();
    assert_eq!(lg.pid, "agent");
    assert_eq!(
        s.acquire_lease(a, None).unwrap_err(),
        SessionError::Leased {
            holder: "agent".into(),
            name: "agent".into(),
            expires_ms: lg.expires_ms
        }
    );

    // breaking needs the owner right; refusals are audited
    let e = f.join(&s, person("editor2", "Ed"));
    assert_eq!(
        s.break_lease(e),
        Err(SessionError::Forbidden { need: Need::Owner })
    );
    assert!(f.events().iter().any(|ev| matches!(
        ev,
        AuditEvent::Refused { pid, need: Need::Owner, .. } if pid == "editor2"
    )));
    let o = f.join(&s, person("owner", "Olga"));
    let broken = s.break_lease(o).unwrap().unwrap();
    assert_eq!(broken.pid, "agent");
    assert!(f.events().iter().any(|ev| matches!(
        ev,
        AuditEvent::LeaseBroken { by, holder, .. } if by == "owner" && holder == "agent"
    )));
    assert_eq!(s.lease().unwrap(), None);
    assert_eq!(s.break_lease(o).unwrap(), None, "nothing left to break");

    // the holder releases; kicked holders lose theirs
    s.acquire_lease(a, None).unwrap();
    assert!(s.release_lease(a).unwrap());
    s.acquire_lease(g, None).unwrap();
    s.kick("agent").unwrap();
    assert_eq!(s.lease().unwrap(), None, "a kicked holder's lease ends");

    // readers can't lease
    f.grant("reader", Rights::read_only());
    let rd = f.join(&s, person("reader", "Rea"));
    assert_eq!(
        s.acquire_lease(rd, None),
        Err(SessionError::Forbidden { need: Need::Edit })
    );
}

#[test]
fn a_lease_survives_leaving_and_keeps_the_session_loaded() {
    let f = fx("lease-cli");
    fs::write(f.path("a.bin"), b"one").unwrap();
    f.grant("agent", Rights::editor());
    // `k2 doc lease` in one call...
    {
        let s = f.open("a.bin");
        let c = s.join_shared(agent("agent"), Stay::UntilIdle).unwrap().conn;
        s.acquire_lease(c, Some(30 * 60_000)).unwrap();
        s.leave(c).unwrap();
    }
    // ...nobody connected for longer than idle_unload_ms: the lease keeps the doc loaded
    f.advance(11 * 60_000);
    f.mgr.tick().unwrap();
    let s = f
        .mgr
        .get_blob(&key("a.bin"))
        .expect("a held lease keeps it loaded");
    // ...`k2 doc write` in the next
    let c = s.join_shared(agent("agent"), Stay::UntilIdle).unwrap().conn;
    assert_eq!(
        s.lease().unwrap().unwrap().pid,
        "agent",
        "still held after leaving"
    );
    let v = s.status().unwrap().version;
    s.write_bytes(c, b"two", v).unwrap();
    assert_eq!(fs::read(f.path("a.bin")).unwrap(), b"two");
    s.release_lease(c).unwrap();
    s.leave(c).unwrap();
    drop(s);
    // without a lease or anyone in it, it unloads when idle
    f.advance(11 * 60_000);
    assert_eq!(f.mgr.tick().unwrap().unloaded, 1);
    assert!(f.mgr.get_blob(&key("a.bin")).is_none());
}

// ---------- write --base ----------

#[test]
fn write_with_a_stale_base_is_refused() {
    let f = fx("stale");
    fs::write(f.path("img.png"), b"\x89PNG one").unwrap();
    f.grant("alice", Rights::editor());
    f.grant("bob", Rights::editor());
    let s = f.open("img.png");
    let (a, b) = (
        f.join(&s, person("alice", "Alice")),
        f.join(&s, agent("bob")),
    );
    let v1 = s.status().unwrap().version;

    let w = s.write_bytes(a, b"\x89PNG two", v1).unwrap();
    assert!(w.changed && w.written);
    assert_eq!(fs::read(f.path("img.png")).unwrap(), b"\x89PNG two");
    // no lease was held, so the write took one for its length and gave it back
    assert_eq!(s.lease().unwrap(), None);
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::LeaseAcquired { pid, auto: true, .. } if pid == "alice"
    )));

    // bob wrote from v1: refused with the newer version, nothing changed
    assert_eq!(
        s.write_bytes(b, b"\x89PNG three", v1),
        Err(SessionError::Stale {
            expected: v1,
            actual: w.version
        })
    );
    assert_eq!(fs::read(f.path("img.png")).unwrap(), b"\x89PNG two");
    assert_eq!(s.status().unwrap().version, w.version);

    // equal bytes record nothing
    let same = s.write_bytes(b, b"\x89PNG two", w.version).unwrap();
    assert!(!same.changed);
    assert_eq!(same.version, w.version);

    // a write's lease is visible while it uploads: alice is told at once
    struct Racing<'a> {
        s: &'a BlobSession,
        other: ConnId,
        base: u64,
        seen: Option<SessionError>,
        left: &'a [u8],
    }
    impl Read for Racing<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.seen.is_none() {
                self.seen = Some(
                    self.s
                        .write_bytes(self.other, b"\x89PNG alice", self.base)
                        .unwrap_err(),
                );
            }
            let n = buf.len().min(self.left.len());
            buf[..n].copy_from_slice(&self.left[..n]);
            self.left = &self.left[n..];
            Ok(n)
        }
    }
    let mut src = Racing {
        s: &s,
        other: a,
        base: w.version,
        seen: None,
        left: b"\x89PNG bob",
    };
    let wb = s.write(b, &mut src, w.version).unwrap();
    assert!(matches!(
        src.seen,
        Some(SessionError::Leased { ref holder, .. }) if holder == "bob"
    ));
    assert_eq!(fs::read(f.path("img.png")).unwrap(), b"\x89PNG bob");
    assert_eq!(s.lease().unwrap(), None, "the write's lease ended with it");
    // the versions record who set each pointer
    let vs = s.versions(a).unwrap_err();
    assert_eq!(
        vs,
        SessionError::Forbidden {
            need: Need::History
        }
    );
    f.grant("alice", Rights::full());
    let vs = s.versions(a).unwrap();
    assert_eq!(vs.len(), 3);
    assert_eq!(vs[1].1.author, "alice");
    assert_eq!(vs[2].1.author, "bob");
    assert_eq!(vs[2].0, wb.version);
    assert_eq!(vs[2].1.reason, BlobReason::Write);
}

// ---------- desktop apps ignore leases ----------

#[test]
fn a_desktop_save_under_someones_lease_keeps_both() {
    let f = fx("conflict");
    let path = f.path("report.pdf");
    fs::write(&path, PDF1).unwrap();
    f.grant("alice", Rights::full());
    let s = f.open("report.pdf");
    let a = f.join(&s, person("alice", "Alice"));
    let v1 = s.status().unwrap().version;
    s.acquire_lease(a, None).unwrap();
    let w = s.write_bytes(a, PDF2, v1).unwrap();
    assert_eq!(fs::read(&path).unwrap(), PDF2);
    assert_eq!(
        s.file_changed().unwrap(),
        DiskCheck::Unchanged,
        "our own write is not an edit"
    );

    // someone's desktop app (which knows nothing of leases) saves over it
    f.advance(1_000);
    app_save(&path, PDF3);
    let out = s.file_changed().unwrap();
    let DiskCheck::Conflict {
        version,
        kept,
        copy,
    } = out
    else {
        panic!("expected a conflict, got {out:?}");
    };
    assert_eq!(kept, w.version, "alice's version is kept");
    let copy = copy.expect("a conflict copy was written");
    assert_eq!(
        copy.file_name().unwrap().to_str().unwrap(),
        "report (conflict Alice 2026-10-27 143001).pdf"
    );

    // the save stays current: the file is untouched, and it is the latest version
    assert_eq!(
        fs::read(&path).unwrap(),
        PDF3,
        "the app's save stays on disk"
    );
    let st = s.status().unwrap();
    assert_eq!(st.version, version);
    assert_eq!(st.current.author, EXTERNAL_PID);
    assert_eq!(st.current.reason, BlobReason::External);
    assert_eq!(
        st.current.conflict_copy.as_deref(),
        copy.file_name().unwrap().to_str()
    );
    assert_eq!(st.disk, Some(DiskState::Clean));
    assert_eq!(read_all(&s, a), PDF3);

    // nothing lost: alice's content is a version and the copy, byte for byte
    assert_eq!(fs::read(&copy).unwrap(), PDF2);
    let mut r = s.read_version(a, kept).unwrap();
    let mut old = Vec::new();
    r.reader.read_to_end(&mut old).unwrap();
    assert_eq!(old, PDF2);
    let vs = s.versions(a).unwrap();
    let reasons: Vec<_> = vs
        .iter()
        .map(|(_, v)| (v.author.as_str(), v.reason))
        .collect();
    assert_eq!(
        reasons,
        [
            ("kit:import", BlobReason::Import),
            ("alice", BlobReason::Write),
            (EXTERNAL_PID, BlobReason::External),
        ]
    );

    // both parties are told
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::BlobConflict { current, kept: k, lease_holder: Some(h), copy: Some(_), .. }
            if *current == version && *k == kept && h == "alice"
    )));
    // alice still holds the lease, but her next write must name the new base
    assert_eq!(
        s.write_bytes(a, b"alice again", w.version),
        Err(SessionError::Stale {
            expected: w.version,
            actual: version
        })
    );
    s.write_bytes(a, b"alice again", version).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"alice again");
}

#[test]
fn an_outside_save_without_a_lease_is_just_a_version() {
    let f = fx("outside");
    let path = f.path("photo.jpg");
    fs::write(&path, b"jpeg-1").unwrap();
    f.grant("alice", Rights::full());
    let s = f.open("photo.jpg");
    let a = f.join(&s, person("alice", "Alice"));
    app_save(&path, b"jpeg-2");
    // found by the stat backup on tick
    f.advance(3_000);
    let rep = f.mgr.tick().unwrap();
    assert_eq!(rep.ingested.len(), 1);
    assert!(rep.conflicts.is_empty());
    let st = s.status().unwrap();
    assert_eq!(st.current.author, EXTERNAL_PID);
    assert!(
        st.current.mtime_ms.is_some(),
        "the file's mtime is recorded"
    );
    assert_eq!(st.current.conflict_copy, None);
    assert_eq!(read_all(&s, a), b"jpeg-2");
    let names: Vec<_> = fs::read_dir(&f.ws).unwrap().flatten().collect();
    assert_eq!(names.len(), 1, "no conflict copy");
}

#[test]
fn a_save_over_a_version_the_file_never_got_keeps_both_even_if_the_copy_fails() {
    let f = fx("unwritten");
    let path = f.path("sheet.xlsx");
    fs::write(&path, b"xlsx-1").unwrap();
    f.grant("agent", Rights::full());
    let s = f.open("sheet.xlsx");
    let g = f.join(&s, agent("agent"));
    let v1 = s.status().unwrap().version;
    // the folder goes read-only: the agent's version can't be written back
    fs::set_permissions(&f.ws, fs::Permissions::from_mode(0o555)).unwrap();
    let w = s.write_bytes(g, b"xlsx-agent", v1).unwrap();
    assert!(w.changed && !w.written, "recorded, not yet on disk");
    assert!(matches!(
        s.status().unwrap().disk,
        Some(DiskState::Dirty { .. })
    ));
    // an app saves in place (the file itself is writable)
    fs::write(&path, b"xlsx-person").unwrap();
    let out = s.file_changed().unwrap();
    fs::set_permissions(&f.ws, fs::Permissions::from_mode(0o755)).unwrap();
    let DiskCheck::Conflict { kept, copy, .. } = out else {
        panic!("expected a conflict, got {out:?}");
    };
    assert_eq!(kept, w.version);
    assert_eq!(
        copy, None,
        "the copy couldn't be written (read-only folder)"
    );
    // still nothing lost: the agent's content is a version; the person's save is current
    let mut r = s.read_version(g, kept).unwrap();
    let mut b = Vec::new();
    r.reader.read_to_end(&mut b).unwrap();
    assert_eq!(b, b"xlsx-agent");
    assert_eq!(read_all(&s, g), b"xlsx-person");
    assert_eq!(fs::read(&path).unwrap(), b"xlsx-person");
    assert_eq!(s.status().unwrap().disk, Some(DiskState::Clean));
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::BlobConflict {
            copy: None,
            lease_holder: None,
            ..
        }
    )));
}

#[test]
fn an_app_saving_an_older_write_of_ours_is_an_echo() {
    let f = fx("echo");
    let path = f.path("a.bin");
    fs::write(&path, b"v1").unwrap();
    f.grant("alice", Rights::full());
    let s = f.open("a.bin");
    let a = f.join(&s, person("alice", "Alice"));
    let w2 = s.write_bytes(a, b"v2", 1).unwrap();
    s.write_bytes(a, b"v3", w2.version).unwrap();
    let n = s.versions(a).unwrap().len();
    // a stale buffer saves v2 back: not an edit; the next write-back restores v3
    app_save(&path, b"v2");
    assert_eq!(s.file_changed().unwrap(), DiskCheck::OwnWrite);
    assert_eq!(s.versions(a).unwrap().len(), n, "no version for an echo");
    let out = s.flush(a).unwrap();
    assert!(out.written);
    assert_eq!(fs::read(&path).unwrap(), b"v3");
    assert!(f
        .events()
        .iter()
        .any(|e| matches!(e, AuditEvent::EchoSuppressed { .. })));
}

/// v0.9.5: a save with the bytes of a kit write made before a person's earlier save is that
/// person's save, never an echo.
#[test]
fn a_save_recreating_a_write_of_ours_after_an_outside_save_is_kept() {
    let f = fx("echo-after-save");
    let path = f.path("a.bin");
    fs::write(&path, b"v1").unwrap();
    f.grant("alice", Rights::full());
    let s = f.open("a.bin");
    let a = f.join(&s, person("alice", "Alice"));
    s.write_bytes(a, b"v2", 1).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"v2");
    // a person saves over it, then saves the old bytes again
    app_save(&path, b"mine");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    app_save(&path, b"v2");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(read_all(&s, a), b"v2");
    s.flush(a).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"v2");
    assert!(!f
        .events()
        .iter()
        .any(|e| matches!(e, AuditEvent::EchoSuppressed { .. })));
}

// ---------- tier 0.5 hook ----------

/// A toy pack whose files are sets of lines: a three-way merge keeps every added line.
struct LineSets;
impl BlobPack for LineSets {
    fn can_merge(&self) -> bool {
        true
    }
    fn merge3_bytes(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        let lines = |b: &[u8]| -> Vec<String> {
            String::from_utf8_lossy(b)
                .lines()
                .map(str::to_owned)
                .collect()
        };
        let base = lines(base);
        let mut out = lines(ours);
        for l in lines(theirs) {
            if !base.contains(&l) && !out.contains(&l) {
                out.push(l);
            }
        }
        BlobMerge::Merged(format!("{}\n", out.join("\n")).into_bytes())
    }
}

#[test]
fn a_pack_that_can_merge_turns_the_conflict_into_a_merge() {
    let f = fx("merge");
    let path = f.path("set.txt");
    fs::write(&path, "a\n").unwrap();
    f.grant("alice", Rights::full());
    let s = f
        .mgr
        .open_blob_file(Arc::new(LineSets), &key("set.txt"), &path, &f.ws)
        .unwrap();
    let a = f.join(&s, person("alice", "Alice"));
    s.acquire_lease(a, None).unwrap();
    s.write_bytes(a, b"a\nb\n", 1).unwrap();
    app_save(&path, b"a\nc\n");
    let DiskCheck::Merged { version } = s.file_changed().unwrap() else {
        panic!("expected a merge");
    };
    assert_eq!(
        fs::read(&path).unwrap(),
        b"a\nb\nc\n",
        "the merge is written back"
    );
    let st = s.status().unwrap();
    assert_eq!(st.version, version);
    assert_eq!(st.current.reason, BlobReason::Merged);
    let reasons: Vec<_> = s
        .versions(a)
        .unwrap()
        .into_iter()
        .map(|(_, v)| v.reason)
        .collect();
    assert_eq!(
        reasons,
        [
            BlobReason::Import,
            BlobReason::Write,
            BlobReason::External,
            BlobReason::Merged
        ]
    );
    assert_eq!(fs::read_dir(&f.ws).unwrap().count(), 1, "no conflict copy");
    assert!(f
        .events()
        .iter()
        .any(|e| matches!(e, AuditEvent::BlobMerged { version: v, .. } if *v == version)));
}

// ---------- versions and restore ----------

#[test]
fn restore_moves_the_pointer_forward() {
    let f = fx("restore");
    let path = f.path("logo.svg");
    fs::write(&path, b"<svg>1</svg>").unwrap();
    f.grant("alice", Rights::full());
    f.grant("ed", Rights::editor());
    let s = f.open("logo.svg");
    let a = f.join(&s, person("alice", "Alice"));
    let v2 = s.write_bytes(a, b"<svg>2</svg>", 1).unwrap().version;
    let v3 = s.write_bytes(a, b"<svg>3</svg>", v2).unwrap().version;

    // restore needs history
    let e = f.join(&s, person("ed", "Ed"));
    assert_eq!(
        s.restore(e, 1),
        Err(SessionError::Forbidden {
            need: Need::History
        })
    );

    let r = s.restore(a, 1).unwrap();
    assert!(r.changed && r.written);
    assert_eq!(r.version, v3 + 1, "a new version, not a rewind");
    assert_eq!(fs::read(&path).unwrap(), b"<svg>1</svg>");
    let vs = s.versions(a).unwrap();
    assert_eq!(vs.len(), 4, "history is never rewritten");
    let last = &vs[3].1;
    assert_eq!(last.reason, BlobReason::Restore);
    assert_eq!(last.restored_from, Some(1));
    assert_eq!(last.author, "alice");
    assert_eq!(last.blob, vs[0].1.blob, "the same blob, not a copy");
    // restoring the current content records nothing
    assert!(!s.restore(a, 1).unwrap().changed);
    // a restore is a write: the lease rule applies
    f.grant("bob", Rights::full());
    let b = f.join(&s, person("bob", "Bob"));
    s.acquire_lease(b, None).unwrap();
    assert!(matches!(s.restore(a, v2), Err(SessionError::Leased { .. })));
}

#[test]
fn named_versions_are_kept_by_retention() {
    let f = fx("named");
    fs::write(f.path("a.bin"), b"one").unwrap();
    f.grant("alice", Rights::full());
    let s = f.open("a.bin");
    let a = f.join(&s, person("alice", "Alice"));
    let v2 = s.write_bytes(a, b"two", 1).unwrap().version;
    let v3 = s.write_bytes(a, b"three", v2).unwrap().version;
    s.name_version(a, v2, "approved").unwrap();
    let id = f.store.find(&key("a.bin")).unwrap().unwrap();
    let now = f.now();
    assert_eq!(
        f.store.delete_blob_version(id, v2, now),
        Err(StoreError::NamedVersion(id, v2))
    );
    assert_eq!(
        f.store.delete_blob_version(id, v3, now),
        Err(StoreError::CurrentVersion(id, v3))
    );
    f.store.delete_blob_version(id, 1, now).unwrap();
    let vs = s.versions(a).unwrap();
    assert_eq!(vs.iter().map(|(v, _)| *v).collect::<Vec<_>>(), [v2, v3]);
    assert_eq!(vs[0].1.name.as_deref(), Some("approved"));
}

// ---------- the blob store ----------

#[test]
fn gc_keeps_referenced_blobs_and_removes_unreferenced_ones() {
    let f = fx("gc");
    fs::write(f.path("a.bin"), b"first").unwrap();
    f.grant("alice", Rights::full());
    let s = f.open("a.bin");
    let a = f.join(&s, person("alice", "Alice"));
    let v2 = s.write_bytes(a, b"second", 1).unwrap().version;
    let first = s.versions(a).unwrap()[0].1.blob;
    let second = s.status().unwrap().current.blob;
    let stray = f.blobs.put_bytes("ws", b"never referenced").unwrap().id;
    let kept = f.blobs.put_bytes("ws", b"retained by a pack").unwrap().id;
    f.blobs.retain("ws", &kept).unwrap();

    // within the grace period nothing goes (a writer may be about to reference it)
    assert_eq!(f.blobs.gc().unwrap().removed, 0);
    f.advance(BlobConfig::default().gc_grace_ms);
    let rep = f.blobs.gc().unwrap();
    assert_eq!(rep.removed, 1, "only the stray blob");
    assert_eq!(rep.bytes, b"never referenced".len() as u64);
    assert!(f.blobs.info("ws", &stray).unwrap().is_none());
    for id in [first, second, kept] {
        assert!(
            f.blobs.info("ws", &id).unwrap().is_some(),
            "{id:?} is referenced"
        );
    }
    assert_eq!(read_all(&s, a), b"second");

    // dropping the last version of a blob makes it collectable, after the grace again
    let id = f.store.find(&key("a.bin")).unwrap().unwrap();
    f.store.delete_blob_version(id, 1, f.now()).unwrap();
    assert_eq!(f.blobs.gc().unwrap().removed, 0);
    f.advance(BlobConfig::default().gc_grace_ms);
    assert_eq!(f.blobs.gc().unwrap().removed, 1);
    assert!(f.blobs.info("ws", &first).unwrap().is_none());
    f.blobs.release("ws", &kept).unwrap();

    // a file with no index row (a crash between storing and indexing) is swept
    let stats = f.mgr.stats().unwrap();
    assert!(stats.blob_bytes > 0 && stats.blob_count >= 2);
    let dir = f.dir.0.join("blobs");
    let space = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .find(|e| e.file_name() != "tmp")
        .unwrap()
        .path();
    let orphan = space.join("ff").join("f".repeat(64));
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::write(&orphan, b"orphan").unwrap();
    assert_eq!(f.blobs.gc().unwrap().orphan_files, 1);
    assert!(!orphan.exists());
    let _ = v2;
}

#[test]
fn keyed_ids_differ_across_spaces_and_a_purge_destroys_the_key() {
    let f = fx("keys");
    let bytes = b"the same file in two workspaces";
    let a = f.blobs.put_bytes("space-a", bytes).unwrap();
    let b = f.blobs.put_bytes("space-b", bytes).unwrap();
    assert_ne!(a.id, b.id, "equal bytes, unrelated ids across spaces");
    let again = f.blobs.put_bytes("space-a", bytes).unwrap();
    assert_eq!(again.id, a.id, "content-addressed within a space");
    assert!(!again.new);
    // hashing without storing gives the same keyed id
    let (same, _) = f.blobs.id_of("space-a", &mut &bytes[..]).unwrap();
    assert_eq!(same, a.id);

    let rep = f.blobs.purge_space_key("space-a").unwrap();
    assert!(rep.key_destroyed);
    assert_eq!(rep.blobs_removed, 1);
    assert_eq!(rep.bytes_removed, bytes.len() as u64);
    assert!(matches!(
        f.blobs.reader("space-a", &a.id),
        Err(BlobError::NotFound(_))
    ));
    // the space gets a fresh key: the same bytes now have a different id
    let fresh = f.blobs.put_bytes("space-a", bytes).unwrap();
    assert_ne!(fresh.id, a.id, "the old key is gone for good");
    assert!(fresh.new);
    // the other space is untouched
    assert_eq!(
        f.blobs.read_to_vec("space-b", &b.id, 1 << 20).unwrap(),
        bytes
    );
    assert!(!f.blobs.purge_space_key("never").unwrap().key_destroyed);
}

#[test]
fn a_damaged_blob_is_never_served_or_written() {
    let f = fx("damaged");
    let path = f.path("a.bin");
    fs::write(&path, b"original bytes").unwrap();
    f.grant("alice", Rights::full());
    let s = f.open("a.bin");
    let a = f.join(&s, person("alice", "Alice"));
    let w = s.write_bytes(a, b"newer bytes!", 1).unwrap();
    // flip a byte of the stored original (same length)
    let first = s.versions(a).unwrap()[0].1.blob;
    let file = fs::read_dir(f.dir.0.join("blobs"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name() != "tmp")
        .flat_map(|e| fs::read_dir(e.path()).unwrap().flatten())
        .flat_map(|e| fs::read_dir(e.path()).unwrap().flatten())
        .find(|e| e.file_name().to_str() == Some(first.to_hex().as_str()))
        .unwrap()
        .path();
    let mut b = fs::read(&file).unwrap();
    b[0] ^= 1;
    fs::write(&file, &b).unwrap();

    assert!(matches!(
        f.blobs.read_to_vec("ws", &first, 1 << 20),
        Err(BlobError::Damaged(_))
    ));
    let mut r = s.read_version(a, 1).unwrap();
    let mut sink = Vec::new();
    let e = r.reader.read_to_end(&mut sink).unwrap_err();
    assert_eq!(
        e.kind(),
        io::ErrorKind::InvalidData,
        "the end of the stream fails"
    );
    assert!(r.reader.read(&mut [0u8; 8]).is_err(), "and keeps failing");
    // restoring it: the version records, but the file is never overwritten with bad bytes
    let out = s.restore(a, 1).unwrap();
    assert!(!out.written);
    assert_eq!(fs::read(&path).unwrap(), b"newer bytes!");
    assert!(f
        .events()
        .iter()
        .any(|e| matches!(e, AuditEvent::DiskError { .. })));
    let _ = w;
}

#[test]
fn files_over_the_blob_limit_are_declined() {
    let mut bc = BlobConfig::default();
    bc.max_blob_bytes = 1024;
    let f = fx_with("limit", bc);
    fs::write(f.path("big.mov"), vec![7u8; 2048]).unwrap();
    let e = f
        .mgr
        .open_blob_file(
            Arc::new(AnyFile),
            &key("big.mov"),
            &f.path("big.mov"),
            &f.ws,
        )
        .err()
        .unwrap();
    assert!(matches!(e, SessionError::Declined(d) if d.code == "too_large"));
    // a writer is told too
    fs::write(f.path("small.mov"), vec![7u8; 10]).unwrap();
    f.grant("alice", Rights::full());
    let s = f.open("small.mov");
    let a = f.join(&s, person("alice", "Alice"));
    assert!(matches!(
        s.write_bytes(a, &[1u8; 4096], 1),
        Err(SessionError::Blob(BlobError::TooLarge { limit: 1024 }))
    ));
    assert_eq!(
        s.lease().unwrap(),
        None,
        "the failed write's lease is released"
    );
    // and a file that grows past it stops being live
    fs::write(f.path("small.mov"), vec![7u8; 4096]).unwrap();
    assert!(matches!(s.file_changed().unwrap(), DiskCheck::Declined(_)));
    assert!(matches!(
        s.write_bytes(a, b"x", 1),
        Err(SessionError::Declined(_))
    ));
}

// ---------- the doc kinds don't mix; restarts ----------

#[test]
fn text_and_blob_docs_never_open_as_each_other() {
    let f = fx("kinds");
    fs::write(f.path("notes.md"), "# hi\n").unwrap();
    fs::write(f.path("pic.png"), b"png").unwrap();
    f.mgr
        .open_file(
            Arc::new(TextFilePack::new("markdown", 1 << 20)),
            &key("notes.md"),
            &f.path("notes.md"),
            &f.ws,
        )
        .unwrap();
    f.open("pic.png");
    assert!(matches!(
        f.mgr.open_blob_file(
            Arc::new(AnyFile),
            &key("notes.md"),
            &f.path("notes.md"),
            &f.ws
        ),
        Err(SessionError::PackMismatch { .. })
    ));
    assert!(matches!(
        f.mgr.open_file(
            Arc::new(TextFilePack::new("markdown", 1 << 20)),
            &key("pic.png"),
            &f.path("pic.png"),
            &f.ws
        ),
        Err(SessionError::PackMismatch { .. })
    ));
    // also from the store, after a restart
    f.mgr.release().unwrap();
    let h = f.host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            f.store.clone(),
            f.dir.0.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(f.store.clone(), f.host.clone(), Config::default())
        .with_blob_store(blobs);
    assert!(matches!(
        mgr.open(
            Arc::new(TextFilePack::new("markdown", 1 << 20)),
            &key("pic.png"),
            None
        ),
        Err(SessionError::PackMismatch { .. })
    ));
    assert!(matches!(
        mgr.open_blob(Arc::new(AnyFile), &key("notes.md"), None),
        Err(SessionError::PackMismatch { .. })
    ));
    // no blob store configured
    let bare = SessionManager::new(f.store.clone(), f.host.clone(), Config::default());
    assert!(matches!(
        bare.open_blob(Arc::new(AnyFile), &key("pic.png"), None),
        Err(SessionError::NoBlobStore)
    ));
}

#[test]
fn reopening_reconciles_with_the_file() {
    let f = fx("reopen");
    let path = f.path("a.pdf");
    fs::write(&path, PDF1).unwrap();
    f.grant("alice", Rights::full());
    {
        let s = f.open("a.pdf");
        let a = f.join(&s, person("alice", "Alice"));
        s.write_bytes(a, PDF2, 1).unwrap();
    }
    f.mgr.release().unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let new_mgr = || {
        let h = f.host.clone();
        let blobs = Arc::new(
            BlobStore::open(
                f.store.clone(),
                f.dir.0.join("blobs"),
                BlobConfig::default(),
                Arc::new(move || h.now_ms()),
            )
            .unwrap(),
        );
        SessionManager::new(f.store.clone(), f.host.clone(), Config::default())
            .with_blob_store(blobs)
    };
    // an untouched file: nothing recorded, nothing written
    let mgr = new_mgr();
    let s = mgr
        .open_blob_file(Arc::new(AnyFile), &key("a.pdf"), &path, &f.ws)
        .unwrap();
    assert_eq!(s.status().unwrap().version, 2);
    assert_eq!(s.status().unwrap().disk, Some(DiskState::Clean));
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    drop(s);
    mgr.release().unwrap();
    // saved while nobody had it open: an outside save on open
    app_save(&path, PDF3);
    let mgr = new_mgr();
    let s = mgr
        .open_blob_file(Arc::new(AnyFile), &key("a.pdf"), &path, &f.ws)
        .unwrap();
    let st = s.status().unwrap();
    assert_eq!(st.version, 3);
    assert_eq!(st.current.author, EXTERNAL_PID);
    assert_eq!(fs::read(&path).unwrap(), PDF3);
}

#[test]
fn a_held_lease_expires_on_tick_and_is_reported() {
    let f = fx("tick");
    fs::write(f.path("a.bin"), b"x").unwrap();
    f.grant("alice", Rights::editor());
    let s = f.open("a.bin");
    let a = f.join(&s, person("alice", "Alice"));
    s.acquire_lease(a, Some(10_000)).unwrap();
    f.advance(10_000);
    let rep = f.mgr.tick().unwrap();
    assert_eq!(rep.leases_expired, [(key("a.bin"), "alice".to_owned())]);
    let stats = f.mgr.stats().unwrap();
    assert_eq!(stats.blob_sessions, 1);
}

/// v0.3.1: naming a version, change or doc that doesn't exist exits 4 (not found), not 6
/// (store unavailable).
#[test]
fn a_missing_version_exits_4_not_6() {
    use k2_mpc_base::tools::session_exit;
    let f = fx("missing");
    fs::write(f.path("a.pdf"), PDF1).unwrap();
    f.grant("alice", Rights::full().with_owner(true));
    let s = f.open("a.pdf");
    let a = f.join(&s, person("alice", "Alice"));
    let err = |r: Result<(), SessionError>| r.expect_err("a missing version must be refused");
    for e in [
        err(s.read_version(a, 99).map(|_| ())),
        err(s.restore(a, 99).map(|_| ())),
        err(s.name_version(a, 99, "x")),
        err(s.purge(a, Some(99)).map(|_| ())),
    ] {
        assert!(
            matches!(e, SessionError::Store(StoreError::NoVersion(_, 99))),
            "{e:?}"
        );
        assert_eq!(session_exit(&e), 4, "{e:?}");
    }
}

// ---------- conflict copies (K-A11Y-5) ----------

const DAY: u64 = 24 * 60 * 60_000;

/// A desktop save under `holder`'s lease over a version `holder` wrote: a conflict whose
/// copy holds `ours`. Returns (the copy, the version it holds, the save's version).
fn conflict(
    f: &Fx,
    s: &BlobSession,
    holder: ConnId,
    ours: &[u8],
    theirs: &[u8],
) -> (PathBuf, u64, u64) {
    let path = s.file_path().unwrap().unwrap();
    let base = s.status().unwrap().version;
    s.acquire_lease(holder, None).unwrap();
    let w = s.write_bytes(holder, ours, base).unwrap();
    assert!(w.written, "control: the holder's version is on disk");
    f.advance(1_000);
    app_save(&path, theirs);
    let out = s.file_changed().unwrap();
    let DiskCheck::Conflict {
        version,
        kept,
        copy,
    } = out
    else {
        panic!("expected a conflict, got {out:?}");
    };
    assert_eq!(kept, w.version);
    s.release_lease(holder).unwrap();
    (copy.expect("a conflict copy was written"), kept, version)
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// K-A11Y-5 (IT `tier0::conflict_copy_naming_cleanup`): the copy is named
/// `<name> (conflict <who> <time>).<ext>`, goes to the host's folder (created), is listed in
/// `status`, and is deleted on `tick` after 30 days (reported), not before.
#[test]
fn conflict_copy_naming_cleanup() {
    let f = fx("a11y5");
    let conflicts = f.conflicts_in(Some("conflicts")).unwrap();
    fs::create_dir_all(f.path("sub")).unwrap();
    let path = f.path("sub/report.pdf");
    fs::write(&path, PDF1).unwrap();
    f.grant("alice", Rights::full());
    let s = f
        .mgr
        .open_blob_file(Arc::new(AnyFile), &key("sub/report.pdf"), &path, &f.ws)
        .unwrap();
    let a = f.join(&s, person("alice", "Alice"));
    assert!(
        !conflicts.exists(),
        "control: the host's folder doesn't exist yet"
    );
    let (copy, kept, version) = conflict(&f, &s, a, PDF2, PDF3);

    // named per K-A11Y-5 (UTC, the host clock), in the host's folder mirroring `sub/`
    assert_eq!(
        copy,
        conflicts.join("sub/report (conflict Alice 2026-10-27 143001).pdf")
    );
    assert_eq!(
        fs::read(&copy).unwrap(),
        PDF2,
        "the copy holds what the save replaced"
    );
    assert_eq!(
        names_in(&f.path("sub")),
        ["report.pdf"],
        "nothing beside the file"
    );
    // recorded relative to the file's folder; the audit event carries the record
    let st = s.status().unwrap();
    assert_eq!(
        st.current.conflict_copy.as_deref(),
        Some("../conflicts/sub/report (conflict Alice 2026-10-27 143001).pdf")
    );
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::BlobConflict { copy: Some(c), .. }
            if c == "../conflicts/sub/report (conflict Alice 2026-10-27 143001).pdf"
    )));
    // listed in status (and for the byte scan)
    assert_eq!(st.conflict_copies.len(), 1);
    let c = &st.conflict_copies[0];
    assert_eq!((&c.path, c.version, c.kept), (&copy, version, kept));
    assert_eq!(c.at_ms, T0 + 1_000);
    assert_eq!(s.conflict_copies(a).unwrap(), vec![copy.clone()]);

    // a second conflict whose name is taken (by a file the kit didn't write) gets ` 2`;
    // the other file is never overwritten, recorded or cleaned up
    let taken = conflicts.join("sub/report (conflict Alice 2026-10-27 143002).pdf");
    fs::write(&taken, b"not the kit's").unwrap();
    let (copy2, _, _) = conflict(&f, &s, a, b"alice two", b"desktop two");
    assert_eq!(
        copy2,
        conflicts.join("sub/report (conflict Alice 2026-10-27 143002) 2.pdf")
    );
    assert_eq!(fs::read(&copy2).unwrap(), b"alice two");
    let at2 = T0 + 2_000;

    // 30 days less a minute: kept
    f.host.now.store(at2 + 30 * DAY - 60_000, Ordering::SeqCst);
    let rep = f.mgr.tick().unwrap();
    assert_eq!(rep.conflict_copies_removed, Vec::<(DocKey, PathBuf)>::new());
    assert!(copy.exists() && copy2.exists());
    // 30 days: both removed, reported, and no longer listed
    f.advance(60_000);
    let rep = f.mgr.tick().unwrap();
    assert_eq!(
        rep.conflict_copies_removed,
        [
            (key("sub/report.pdf"), copy.clone()),
            (key("sub/report.pdf"), copy2.clone())
        ]
    );
    assert!(!copy.exists() && !copy2.exists());
    assert_eq!(
        fs::read(&taken).unwrap(),
        b"not the kit's",
        "never the kit's to delete"
    );
    assert_eq!(s.status().unwrap().conflict_copies, Vec::new());
    assert_eq!(s.conflict_copies(a).unwrap(), Vec::<PathBuf>::new());
    // the versions stay: the cleanup removes files, never history
    let mut r = s.read_version(a, kept).unwrap();
    let mut b = Vec::new();
    r.reader.read_to_end(&mut b).unwrap();
    assert_eq!(b, PDF2);
    // and the next tick has nothing to do
    f.advance(DAY);
    assert!(f.mgr.tick().unwrap().conflict_copies_removed.is_empty());
}

/// Without `Host::conflict_dir` (the default), the copy goes beside the file, recorded by
/// its plain name, as before v0.4.0; with it, the folder is the host's.
#[test]
fn the_conflict_folder_is_the_hosts_choice_or_beside_the_file() {
    let f = fx("a11y5-dir");
    fs::write(f.path("beside.bin"), b"one").unwrap();
    fs::write(f.path("hosted.bin"), b"one").unwrap();
    f.grant("alice", Rights::full());
    let a_s = f.open("beside.bin");
    let a = f.join(&a_s, person("alice", "Alice"));
    let (copy, _, _) = conflict(&f, &a_s, a, b"two", b"three");
    assert_eq!(copy.parent().unwrap(), f.ws);
    assert_eq!(
        a_s.status().unwrap().current.conflict_copy.as_deref(),
        copy.file_name().unwrap().to_str()
    );
    let held = f.conflicts_in(Some("held")).unwrap();
    let b_s = f.open("hosted.bin");
    let b = f.join(&b_s, person("alice", "Alice"));
    let (copy, _, _) = conflict(&f, &b_s, b, b"two", b"three");
    assert_eq!(copy.parent().unwrap(), held);
    assert_eq!(names_in(&held).len(), 1);
    assert_eq!(
        names_in(&f.ws),
        [
            "beside (conflict Alice 2026-10-27 143001).bin",
            "beside.bin",
            "held",
            "hosted.bin"
        ]
    );
    // a folder that can't be made: no copy (a disk error), the content is still a version
    f.conflicts_in(Some("hosted.bin/nope"));
    let before = b_s.status().unwrap().version;
    b_s.acquire_lease(b, None).unwrap();
    f.advance(1_000);
    app_save(&f.path("hosted.bin"), b"four");
    let out = b_s.file_changed().unwrap();
    assert!(
        matches!(out, DiskCheck::Conflict { copy: None, kept, .. } if kept == before),
        "{out:?}"
    );
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::DiskError { error, .. } if error.contains("conflict-copy folder")
    )));
}

/// A copy a person edited (its bytes no longer match its version) is never deleted, even
/// with the same size; one whose version is named is kept too.
#[test]
fn an_edited_or_named_conflict_copy_is_never_deleted() {
    let f = fx("a11y5-keep");
    f.conflicts_in(Some("conflicts"));
    for n in ["edited.bin", "named.bin", "plain.bin"] {
        fs::write(f.path(n), b"base").unwrap();
    }
    f.grant("alice", Rights::full());
    let mut copies = Vec::new();
    let mut sessions = Vec::new();
    for n in ["edited.bin", "named.bin", "plain.bin"] {
        let s = f.open(n);
        let a = f.join(&s, person("alice", "Alice"));
        let (copy, kept, _) = conflict(&f, &s, a, b"alice's", b"desktop");
        copies.push((copy, kept));
        sessions.push((s, a));
    }
    // the person edits one copy in place (same length), and names another's version
    fs::write(&copies[0].0, b"ALICE's").unwrap();
    sessions[1]
        .0
        .name_version(sessions[1].1, copies[1].1, "keep me")
        .unwrap();
    f.advance(31 * DAY);
    let rep = f.mgr.tick().unwrap();
    assert_eq!(
        rep.conflict_copies_removed,
        [(key("plain.bin"), copies[2].0.clone())],
        "only the untouched, unnamed copy goes"
    );
    assert_eq!(
        fs::read(&copies[0].0).unwrap(),
        b"ALICE's",
        "the edit is kept"
    );
    assert_eq!(fs::read(&copies[1].0).unwrap(), b"alice's");
    // still kept on later ticks (the edited one isn't re-hashed until it changes)
    f.advance(DAY);
    assert!(f.mgr.tick().unwrap().conflict_copies_removed.is_empty());
    assert!(copies[0].0.exists() && copies[1].0.exists());
    // edited back to the version's bytes, it is the kit's copy again
    fs::write(&copies[0].0, b"alice's").unwrap();
    f.advance(DAY);
    assert_eq!(
        f.mgr.tick().unwrap().conflict_copies_removed,
        [(key("edited.bin"), copies[0].0.clone())]
    );
    // a symlink in a copy's place is never followed or removed
    let target = f.path("precious.txt");
    fs::write(&target, b"alice's").unwrap();
    std::os::unix::fs::symlink(&target, &copies[0].0).unwrap();
    f.advance(DAY);
    assert!(f.mgr.tick().unwrap().conflict_copies_removed.is_empty());
    assert_eq!(fs::read(&target).unwrap(), b"alice's");
}

/// Records written by v0.3.x (a plain name: the copy beside the file) still resolve after
/// the host chooses a folder: `conflict_copies`, `status`, the purge byte scan and the
/// cleanup all find them.
#[test]
fn old_records_beside_the_file_still_resolve() {
    let f = fx("a11y5-old");
    fs::write(f.path("a.bin"), b"base").unwrap();
    fs::write(f.path("b.bin"), b"base").unwrap();
    f.grant("alice", Rights::full().with_owner(true));
    // the v0.3.x layout: no host folder, the record is the copy's file name
    let (old_a, old_b) = {
        let a_s = f.open("a.bin");
        let a = f.join(&a_s, person("alice", "Alice"));
        let b_s = f.open("b.bin");
        let b = f.join(&b_s, person("alice", "Alice"));
        let ca = conflict(&f, &a_s, a, b"alice a", b"desktop a");
        let cb = conflict(&f, &b_s, b, b"alice b", b"desktop b");
        a_s.leave(a).unwrap();
        b_s.leave(b).unwrap();
        (ca, cb)
    };
    assert_eq!(old_a.0.parent().unwrap(), f.ws);
    f.advance(f.mgr.config().idle_unload_ms + 1);
    f.mgr.tick().unwrap();
    assert_eq!(f.mgr.stats().unwrap().blob_sessions, 0, "both unloaded");

    // now the host keeps copies elsewhere; the old ones are still found
    let conflicts = f.conflicts_in(Some("conflicts")).unwrap();
    let a_s = f.open("a.bin");
    let a = f.join(&a_s, person("alice", "Alice"));
    let b_s = f.open("b.bin");
    let b = f.join(&b_s, person("alice", "Alice"));
    assert_eq!(a_s.conflict_copies(a).unwrap(), vec![old_a.0.clone()]);
    assert_eq!(a_s.status().unwrap().conflict_copies[0].path, old_a.0);
    // a new conflict on b goes to the host's folder; both are listed
    let new_b = conflict(&f, &b_s, b, b"alice b2", b"desktop b2");
    assert_eq!(new_b.0.parent().unwrap(), conflicts);
    assert_eq!(
        b_s.conflict_copies(b).unwrap(),
        vec![old_b.0.clone(), new_b.0.clone()]
    );
    // the purge byte scan finds the old copy (its bytes are the purged version's)
    let out = a_s.purge(a, Some(old_a.1)).unwrap();
    assert_eq!(out.counts.conflict_copies, 1);
    assert!(!old_a.0.exists());
    // and the cleanup removes the old one beside the file after 30 days
    f.advance(30 * DAY);
    let rep = f.mgr.tick().unwrap();
    assert_eq!(
        rep.conflict_copies_removed,
        [
            (key("b.bin"), old_b.0.clone()),
            (key("b.bin"), new_b.0.clone())
        ]
    );
}

/// K-T0-9: the purge byte scan covers copies in the host's folder.
#[test]
fn purge_scan_covers_the_conflict_folder() {
    let f = fx("a11y5-purge");
    let conflicts = f.conflicts_in(Some("conflicts")).unwrap();
    fs::write(f.path("p.bin"), b"base").unwrap();
    f.grant("alice", Rights::full().with_owner(true));
    let s = f.open("p.bin");
    let a = f.join(&s, person("alice", "Alice"));
    let (copy, kept, _) = conflict(&f, &s, a, b"secret 0123456789", b"desktop");
    let (copy2, kept2, _) = conflict(&f, &s, a, b"other 0123456789", b"desktop 2");
    assert_eq!(copy.parent().unwrap(), conflicts);
    // the person edits the second copy: the purge leaves it and says so
    fs::write(&copy2, b"edited by hand").unwrap();
    let out = s.purge(a, Some(kept)).unwrap();
    assert_eq!(out.counts.conflict_copies, 1);
    assert!(!copy.exists(), "the purged version's copy is gone");
    let out = s.purge(a, Some(kept2)).unwrap();
    assert_eq!(out.counts.conflict_copies, 0);
    assert!(
        out.not_reached
            .contains(&k2_mpc_base::session::NotReached::ConflictCopies(1)),
        "{:?}",
        out.not_reached
    );
    assert_eq!(fs::read(&copy2).unwrap(), b"edited by hand");
}

// ---------- the log (any file) ----------

/// `BlobSession::log`: one entry per version, oldest first, with who, when, what it was,
/// for whom, its name, purged, and its conflict copy. Needs history, as `versions`.
#[test]
fn blob_log_lists_every_version_oldest_first() {
    let f = fx("log");
    f.conflicts_in(Some("conflicts"));
    let path = f.path("deck.key");
    fs::write(&path, b"deck 1").unwrap();
    f.grant("alice", Rights::full().with_owner(true));
    f.grant("aide", Rights::editor());
    f.grant("reader", Rights::read_only());
    let s = f.open("deck.key");
    let a = f.join(&s, person("alice", "Alice"));
    let g = f.join(&s, agent("aide").on_behalf_of("alice"));
    // an agent writes for alice; a desktop save conflicts with it; alice restores the
    // import, names it, then purges the agent's version
    f.advance(1_000);
    let w = s.write_bytes(g, b"deck 2", 1).unwrap();
    let (copy, kept, saved) = conflict(&f, &s, a, b"deck 3", b"deck 4");
    f.advance(1_000);
    let r = s.restore(a, 1).unwrap();
    s.name_version(a, r.version, "back to the start").unwrap();
    s.purge(a, Some(w.version)).unwrap();

    let log = s.log(a).unwrap();
    let row = |e: &BlobLogEntry| {
        (
            e.id,
            e.kind,
            e.of,
            e.author.clone(),
            e.via.clone(),
            e.on_behalf_of.clone(),
            e.name.clone(),
            e.redacted,
        )
    };
    let o = |s: &str| Some(s.to_owned());
    assert_eq!(
        log.iter().map(row).collect::<Vec<_>>(),
        [
            (
                1,
                BlobLogKind::Import,
                None,
                "kit:import".into(),
                "disk".into(),
                None,
                None,
                false
            ),
            (
                w.version,
                BlobLogKind::Write,
                None,
                "aide".into(),
                "cli".into(),
                o("alice"),
                None,
                true
            ),
            (
                kept,
                BlobLogKind::Write,
                None,
                "alice".into(),
                "tab".into(),
                None,
                None,
                false
            ),
            (
                saved,
                BlobLogKind::ConflictKept,
                None,
                EXTERNAL_PID.into(),
                "disk".into(),
                None,
                None,
                false
            ),
            (
                r.version,
                BlobLogKind::Restore,
                Some(1),
                "alice".into(),
                "tab".into(),
                None,
                o("back to the start"),
                false
            ),
        ]
    );
    assert_eq!(log[3].conflict_copy.as_deref(), Some(copy.as_path()));
    assert!(log
        .iter()
        .filter(|e| e.id != saved)
        .all(|e| e.conflict_copy.is_none()));
    assert_eq!((log[0].at_ms, log[1].at_ms), (T0, T0 + 1_000));
    assert_eq!(log[1].author_name, "aide");
    assert_eq!(log[1].size, 6, "a purged entry keeps its size");
    assert_eq!(log[3].size, 6);
    assert!(log[3].mtime_ms.is_some() && log[2].mtime_ms.is_none());
    // the same versions, in the same order, as `versions`
    let ids: Vec<u64> = s.versions(a).unwrap().iter().map(|(id, _)| *id).collect();
    assert_eq!(log.iter().map(|e| e.id).collect::<Vec<_>>(), ids);
    // rights: history, as `versions`
    let rd = f.join(&s, person("reader", "Reader"));
    assert_eq!(
        s.log(rd),
        Err(SessionError::Forbidden {
            need: Need::History
        })
    );

    // tier 0.5: the save and its merge
    fs::write(f.path("set.txt"), "a\n").unwrap();
    let m = f
        .mgr
        .open_blob_file(
            Arc::new(LineSets),
            &key("set.txt"),
            &f.path("set.txt"),
            &f.ws,
        )
        .unwrap();
    let a = f.join(&m, person("alice", "Alice"));
    m.acquire_lease(a, None).unwrap();
    m.write_bytes(a, b"a\nb\n", 1).unwrap();
    app_save(&f.path("set.txt"), b"a\nc\n");
    assert!(matches!(
        m.file_changed().unwrap(),
        DiskCheck::Merged { .. }
    ));
    let kinds: Vec<(BlobLogKind, String)> = m
        .log(a)
        .unwrap()
        .into_iter()
        .map(|e| (e.kind, e.author))
        .collect();
    assert_eq!(
        kinds,
        [
            (BlobLogKind::Import, "kit:import".to_owned()),
            (BlobLogKind::Write, "alice".to_owned()),
            (BlobLogKind::External, EXTERNAL_PID.to_owned()),
            (BlobLogKind::Merged, "kit:merge".to_owned()),
        ]
    );
}

// ---------- on-behalf writes and their approvals (K-ID-10, K-ID-11, K-T0-9) ----------

/// The host restarts: every handle goes, and the store file and blob folder are opened again.
fn restart(f: Fx) -> Fx {
    let Fx {
        dir,
        ws,
        host,
        store,
        blobs,
        mgr,
    } = f;
    mgr.release().unwrap();
    drop(mgr);
    drop(blobs);
    assert_eq!(Arc::strong_count(&store), 1, "nothing else holds the store");
    drop(store);
    let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.0.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs.clone());
    Fx {
        dir,
        ws,
        host,
        store,
        blobs,
        mgr,
    }
}

/// alice (person), aide (her assistant, an editor), bob (another person, full rights) and
/// own (an owner) on `name`, a PDF created with [`PDF1`].
fn approval_fx(tag: &str, name: &str) -> Fx {
    let f = fx(tag);
    fs::write(f.path(name), PDF1).unwrap();
    f.grant("alice", Rights::full());
    f.grant("aide", Rights::editor());
    f.grant("bob", Rights::full());
    f.grant("own", Rights::full().with_owner(true));
    f
}

fn aide() -> Participant {
    agent("aide").on_behalf_of("alice")
}

fn approval_events(f: &Fx) -> Vec<AuditEvent> {
    f.events()
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                AuditEvent::Approval { .. }
                    | AuditEvent::Refused {
                        action: Action::Approve,
                        ..
                    }
            )
        })
        .collect()
}

/// K-T0-9 (IT `tier0::on_behalf_blob_write_pending_approval`, K-ID-10 for tier 0): an
/// assistant's write (and restore) on a person's behalf applies at once, not held, and
/// records a `Pending` approval `{by, at}` empty, the agent as author; a person's own write
/// records none, and so does a write of equal bytes (no version). The log, blame and `who`
/// carry it.
#[test]
fn on_behalf_blob_write_pending_approval() {
    let f = approval_fx("obo", "a.pdf");
    let s = f.open("a.pdf");
    let a = f.join(&s, person("alice", "Alice"));
    let g = f.join(&s, aide());
    f.advance(1_000);
    let w = s.write_bytes(g, PDF2, 1).unwrap();
    // applied at once: current, on disk, readable by everyone
    assert!(w.changed && w.written);
    assert_eq!(s.status().unwrap().version, w.version);
    assert_eq!(fs::read(f.path("a.pdf")).unwrap(), PDF2);
    assert_eq!(read_all(&s, a), PDF2);
    let pending = Approval::pending("aide", "alice");
    assert_eq!(s.approval(a, w.version).unwrap(), Some(pending.clone()));
    assert_eq!(
        (pending.state, pending.by.clone(), pending.at_ms),
        (ApprovalState::Pending, None, None)
    );
    // the agent's equal bytes: nothing recorded
    let same = s.write_bytes(g, PDF2, w.version).unwrap();
    assert!(!same.changed);
    // alice's own write: no approval; the agent's restore: pending again
    f.advance(1_000);
    let mine = s.write_bytes(a, PDF3, w.version).unwrap();
    assert_eq!(s.approval(a, mine.version).unwrap(), None);
    f.advance(1_000);
    f.grant("aide", Rights::full()); // restore needs history
    let r = s.restore(g, 1).unwrap();
    assert_eq!(s.approval(a, r.version).unwrap(), Some(pending.clone()));
    assert_eq!(
        s.approvals(a)
            .unwrap()
            .into_iter()
            .map(|(v, x)| (v, x.state))
            .collect::<Vec<_>>(),
        [
            (w.version, ApprovalState::Pending),
            (r.version, ApprovalState::Pending)
        ]
    );
    // log: each version's approval state; blame: the current version's
    let log = s.log(a).unwrap();
    assert_eq!(
        log.iter()
            .map(|e| (e.id, e.on_behalf_of.clone(), e.approval))
            .collect::<Vec<_>>(),
        [
            (1, None, None),
            (
                w.version,
                Some("alice".to_owned()),
                Some(ApprovalState::Pending)
            ),
            (mine.version, None, None),
            (
                r.version,
                Some("alice".to_owned()),
                Some(ApprovalState::Pending)
            ),
        ]
    );
    let b = s.blame(a).unwrap();
    assert_eq!(
        (b.current.author.as_str(), b.approval),
        ("aide", Some(ApprovalState::Pending))
    );
    // who: "aide for alice"
    let who = s.roster(a).unwrap();
    let e = who.iter().find(|e| e.participant.pid == "aide").unwrap();
    assert_eq!(e.participant.on_behalf_of.as_deref(), Some("alice"));
    // nothing settled, nothing refused yet
    assert!(approval_events(&f).is_empty());
}

/// K-ID-11 for tier 0 (approver identity): only the named person settles it. The agent, a
/// different person and an owner are each refused with `Need::Approver` and an audit event,
/// and the record stays pending; the person approves; again the same way is a no-op, the
/// other way `BadApproval`; `Pending` is no target; a version made on nobody's behalf (or
/// none at all) is `NoApproval` (exit 4). After a restart (the store reopened) a pending
/// approval is still there and still only the person can settle it.
#[test]
fn on_behalf_blob_approval_only_by_person() {
    let f = approval_fx("approver", "a.pdf");
    let (v2, v3) = {
        let s = f.open("a.pdf");
        let a = f.join(&s, person("alice", "Alice"));
        let g = f.join(&s, aide());
        let b = f.join(&s, person("bob", "Bob"));
        let o = f.join(&s, person("own", "Owner"));
        let v2 = s.write_bytes(g, PDF2, 1).unwrap().version;
        let v3 = s.write_bytes(g, PDF3, v2).unwrap().version;
        for (c, who) in [(g, "aide"), (b, "bob"), (o, "own")] {
            assert_eq!(
                s.set_approval(c, v2, ApprovalState::Approved),
                Err(SessionError::Forbidden {
                    need: Need::Approver
                }),
                "{who}"
            );
            assert_eq!(
                s.set_approval(c, v2, ApprovalState::Declined),
                Err(SessionError::Forbidden {
                    need: Need::Approver
                }),
                "{who}"
            );
        }
        let refused: Vec<String> = approval_events(&f)
            .into_iter()
            .map(|e| match e {
                AuditEvent::Refused { pid, need, .. } => {
                    assert_eq!(need, Need::Approver);
                    pid
                }
                e => panic!("unexpected {e:?}"),
            })
            .collect();
        assert_eq!(refused, ["aide", "aide", "bob", "bob", "own", "own"]);
        assert_eq!(
            s.approval(a, v2).unwrap().unwrap().state,
            ApprovalState::Pending
        );
        // the person approves
        f.advance(5_000);
        let at = f.now();
        let ok = s.set_approval(a, v2, ApprovalState::Approved).unwrap();
        let want = Approval::pending("aide", "alice").settled(ApprovalState::Approved, "alice", at);
        assert_eq!(ok.approval, want);
        assert_eq!((ok.version, ok.restore_to, ok.later.len()), (v2, None, 0));
        assert!(matches!(
            approval_events(&f).last(),
            Some(AuditEvent::Approval { seq, pid, state: ApprovalState::Approved, .. })
                if *seq == v2 && pid == "alice"
        ));
        let n = approval_events(&f).len();
        // again the same way: unchanged, no new event; the other way, or Pending: refused
        f.advance(5_000);
        assert_eq!(
            s.set_approval(a, v2, ApprovalState::Approved)
                .unwrap()
                .approval,
            want
        );
        assert_eq!(approval_events(&f).len(), n);
        assert!(matches!(
            s.set_approval(a, v2, ApprovalState::Declined),
            Err(SessionError::BadApproval(_))
        ));
        assert!(matches!(
            s.set_approval(a, v3, ApprovalState::Pending),
            Err(SessionError::BadApproval(_))
        ));
        assert_eq!(s.approval(a, v2).unwrap(), Some(want));
        // no approval: the import (nobody's behalf), a version that doesn't exist; exit 4
        for v in [1, 99] {
            let e = s.set_approval(a, v, ApprovalState::Approved).unwrap_err();
            assert_eq!(e, SessionError::NoApproval { seq: v });
            assert_eq!(k2_mpc_base::tools::session_exit(&e), 4);
        }
        // reading: the agent and the person see their own; bob without history may not
        f.grant("bob", Rights::editor());
        assert!(s.approval(g, v3).unwrap().is_some());
        assert_eq!(
            s.approval(b, v3),
            Err(SessionError::Forbidden {
                need: Need::History
            })
        );
        assert!(s.approvals(b).unwrap().is_empty());
        assert_eq!(s.approvals(g).unwrap().len(), 2);
        f.grant("bob", Rights::full());
        (v2, v3)
    };

    // restart: the store reopened from its file
    let f = restart(f);
    let s = f.open("a.pdf");
    let a = f.join(&s, person("alice", "Alice"));
    let g = f.join(&s, aide());
    let b = f.join(&s, person("bob", "Bob"));
    let o = f.join(&s, person("own", "Owner"));
    assert_eq!(
        s.approval(a, v3).unwrap(),
        Some(Approval::pending("aide", "alice"))
    );
    assert_eq!(
        s.approval(a, v2).unwrap().unwrap().state,
        ApprovalState::Approved
    );
    let before = approval_events(&f).len();
    for c in [g, b, o] {
        assert_eq!(
            s.set_approval(c, v3, ApprovalState::Declined),
            Err(SessionError::Forbidden {
                need: Need::Approver
            })
        );
    }
    assert_eq!(approval_events(&f).len(), before + 3);
    assert_eq!(
        s.approval(a, v3).unwrap().unwrap().state,
        ApprovalState::Pending
    );
    let d = s.set_approval(a, v3, ApprovalState::Declined).unwrap();
    assert_eq!(d.approval.state, ApprovalState::Declined);
    assert_eq!(d.approval.by.as_deref(), Some("alice"));
}

/// K-HIST-19 for tier 0 (mirrors `decline_then_revert_after_restart`): a declined version
/// stays; after a restart the person declines it and is told the version to restore and
/// the later versions a restore would also replace; nothing is restored on its own; the
/// person's explicit restore brings the content back as a new version.
#[test]
fn decline_then_restore_after_restart() {
    let f = approval_fx("decline", "a.pdf");
    let v2 = {
        let s = f.open("a.pdf");
        let g = f.join(&s, aide());
        let b = f.join(&s, person("bob", "Bob"));
        let v2 = s.write_bytes(g, PDF2, 1).unwrap().version;
        f.advance(1_000);
        s.write_bytes(b, PDF3, v2).unwrap();
        v2
    };
    let f = restart(f);
    f.advance(DAY);
    let s = f.open("a.pdf");
    let a = f.join(&s, person("alice", "Alice"));
    assert_eq!(
        s.approval(a, v2).unwrap().unwrap().state,
        ApprovalState::Pending
    );
    let d = s.set_approval(a, v2, ApprovalState::Declined).unwrap();
    assert_eq!(d.restore_to, Some(1), "the version before the agent's");
    assert_eq!(d.later, vec![v2 + 1], "bob's write came after it");
    // not restored on its own: bob's version is current, the declined one stays
    assert_eq!(s.status().unwrap().version, v2 + 1);
    assert_eq!(read_all(&s, a), PDF3);
    let log = s.log(a).unwrap();
    let e = log.iter().find(|e| e.id == v2).unwrap();
    assert_eq!(
        (e.redacted, e.approval),
        (false, Some(ApprovalState::Declined))
    );
    // re-declining says the same
    assert_eq!(s.set_approval(a, v2, ApprovalState::Declined).unwrap(), d);
    // the person's explicit restore
    let r = s.restore(a, d.restore_to.unwrap()).unwrap();
    assert!(r.changed);
    assert_eq!(read_all(&s, a), PDF1);
    assert_eq!(fs::read(f.path("a.pdf")).unwrap(), PDF1);
    let last = s.log(a).unwrap().pop().unwrap();
    assert_eq!(
        (last.kind, last.of, last.author.as_str(), last.approval),
        (BlobLogKind::Restore, Some(1), "alice", None)
    );

    // the last version declined: nothing later; the one before it purged: nothing to restore
    let s2 = {
        fs::write(f.path("b.pdf"), PDF1).unwrap();
        f.open("b.pdf")
    };
    let a2 = f.join(&s2, person("alice", "Alice"));
    let g2 = f.join(&s2, aide());
    let o2 = f.join(&s2, person("own", "Owner"));
    s2.write_bytes(a2, PDF2, 1).unwrap();
    let w = s2.write_bytes(g2, PDF3, 2).unwrap().version;
    s2.purge(o2, Some(2)).unwrap();
    let d = s2.set_approval(a2, w, ApprovalState::Declined).unwrap();
    assert_eq!((d.restore_to, d.later), (None, vec![]));
}

/// Format 3 → 4 on a real store written by v0.4.0 (`tests/fixtures/store-format3-v040`,
/// made by its `generate.rs.txt`): it opens in place, the on-behalf blob versions (one of them
/// purged by v0.4.0) get their `Pending` approvals, the text doc's approval is untouched,
/// the content reads back, and only the named person can settle them.
#[test]
fn a_v040_store_upgrades_to_format_4_in_place() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/store-format3-v040");
    // the fixture's store and blob folder, beside this world's (closed first)
    let Fx {
        dir,
        ws,
        host,
        store,
        blobs,
        mgr,
    } = fx("format4");
    mgr.release().unwrap();
    drop((mgr, blobs, store));
    let db = dir.0.join("v040.redb");
    fs::write(
        &db,
        unrle(&fs::read(fixture.join("docs.redb.rle")).unwrap()),
    )
    .unwrap();
    copy_dir(&fixture.join("blobs"), &dir.0.join("v040-blobs"));
    // the raw stamp before: 3
    assert_eq!(raw_format(&db), 3);
    let store = Arc::new(DocStore::open(&db).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.0.join("v040-blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(store.clone(), host.clone(), Config::default())
        .with_blob_store(blobs.clone());
    let f = Fx {
        dir,
        ws,
        host,
        store,
        blobs,
        mgr,
    };
    f.grant("alice", Rights::full());
    f.grant("aide", Rights::editor());
    f.grant("bob", Rights::full());
    f.grant("own", Rights::full().with_owner(true));
    let s = f
        .mgr
        .open_blob(Arc::new(AnyFile), &key("report.pdf"), None)
        .unwrap();
    let a = f.join(&s, person("alice", "Alice"));
    assert_eq!(read_all(&s, a), b"%PDF v4 by aide for alice\n");
    let log = s.log(a).unwrap();
    assert_eq!(
        log.iter()
            .map(|e| (e.id, e.author.as_str(), e.redacted, e.approval))
            .collect::<Vec<_>>(),
        [
            (1, "kit:import", false, None),
            (2, "aide", true, Some(ApprovalState::Pending)),
            (3, "alice", false, None),
            (4, "aide", false, Some(ApprovalState::Pending)),
        ]
    );
    // the text doc's approval (its own table) is as v0.4.0 left it
    let t = f
        .mgr
        .open(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &key("notes.md"),
            None,
        )
        .unwrap();
    let ta = t.join(person("alice", "Alice"), Stay::Held).unwrap().conn;
    assert_eq!(
        t.approvals(ta).unwrap(),
        vec![(2, Approval::pending("aide", "alice"))]
    );
    // only alice settles the backfilled approvals
    let g = f.join(&s, aide());
    let b = f.join(&s, person("bob", "Bob"));
    let o = f.join(&s, person("own", "Owner"));
    for c in [g, b, o] {
        assert_eq!(
            s.set_approval(c, 4, ApprovalState::Approved),
            Err(SessionError::Forbidden {
                need: Need::Approver
            })
        );
    }
    assert_eq!(
        s.set_approval(a, 4, ApprovalState::Approved)
            .unwrap()
            .approval
            .state,
        ApprovalState::Approved
    );
    let d = s.set_approval(a, 2, ApprovalState::Declined).unwrap();
    assert_eq!((d.restore_to, d.later), (Some(1), vec![3, 4]));
    // stamped with the current format on disk (4 then; 5 since provenance, a further
    // in-place step)
    drop((s, t));
    let Fx {
        dir,
        store,
        blobs,
        mgr,
        ..
    } = f;
    mgr.release().unwrap();
    drop((mgr, blobs, store));
    assert_eq!(
        raw_format(&dir.0.join("v040.redb")),
        k2_mpc_base::store::STORE_FORMAT
    );
}

/// The store's raw format stamp (`meta.store_format`), read with redb directly.
fn raw_format(db: &Path) -> u64 {
    use redb::{Database, TableDefinition};
    const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
    let d = Database::open(db).unwrap();
    let r = d.begin_read().unwrap();
    let t = r.open_table(META).unwrap();
    let v = t.get("store_format").unwrap().unwrap().value();
    v
}

/// Decode the fixture's run-length encoding: `MPCRLE1\n`, then records `0, len u32le,
/// bytes` (literal) or `1, byte, count u32le` (a run).
fn unrle(b: &[u8]) -> Vec<u8> {
    assert!(b.starts_with(b"MPCRLE1\n"), "not an MPCRLE1 file");
    let u32le = |x: &[u8]| u32::from_le_bytes(x[..4].try_into().unwrap()) as usize;
    let (mut out, mut i) = (Vec::new(), 8);
    while i < b.len() {
        match b[i] {
            0 => {
                let n = u32le(&b[i + 1..]);
                out.extend_from_slice(&b[i + 5..i + 5 + n]);
                i += 5 + n;
            }
            1 => {
                out.resize(out.len() + u32le(&b[i + 2..]), b[i + 1]);
                i += 6;
            }
            t => panic!("bad record tag {t} at {i}"),
        }
    }
    out
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

// ---------- racy clean (v0.9.5): an equal stat can't vouch for a file saved inside its mtime tick

/// An in-place save (same inode, same size) that leaves the file's mtime at `mtime`: what a
/// second save inside one mtime tick looks like, made deterministic.
fn save_in_place_at(path: &Path, bytes: &[u8], mtime: std::time::SystemTime) {
    fs::write(path, bytes).unwrap();
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

/// An mtime the kit can't trust yet (in the future, like a share whose clock runs ahead):
/// every stat it takes of the file is racy.
fn racy_mtime() -> std::time::SystemTime {
    std::time::SystemTime::now() + std::time::Duration::from_secs(3600)
}

/// A file the kit has synced, its mtime set to `at` before the kit first looks.
fn racy_file(f: &Fx, name: &str, bytes: &[u8], at: std::time::SystemTime) -> PathBuf {
    let path = f.path(name);
    save_in_place_at(&path, bytes, at);
    path
}

/// (a) A person's same-size save inside the tick of the kit's last look, then a kit version:
/// the write-back's check finds the save, which stays current (and on disk); nothing of it
/// is overwritten.
#[test]
fn racy_a_save_inside_the_tick_is_never_overwritten_by_a_write_back() {
    let f = fx("racy-wb");
    let at = racy_mtime();
    let path = racy_file(&f, "sheet.xlsx", b"orig-01", at);
    f.grant("agent", Rights::full());
    let s = f.open("sheet.xlsx");
    let g = f.join(&s, agent("agent"));
    let v1 = s.status().unwrap().version;
    save_in_place_at(&path, b"pers-01", at);
    let w = s.write_bytes(g, b"agent-version", v1).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        b"pers-01",
        "the person's save is never overwritten"
    );
    assert_eq!(read_all(&s, g), b"pers-01", "the save is current");
    let mut r = s.read_version(g, w.version).unwrap();
    let mut b = Vec::new();
    r.reader.read_to_end(&mut b).unwrap();
    assert_eq!(b, b"agent-version", "the kit's version is kept");
}

/// (b) The stat poll on tick finds a same-size save inside the tick.
#[test]
fn racy_the_poll_finds_a_save_inside_the_tick() {
    let f = fx("racy-poll");
    let at = racy_mtime();
    let path = racy_file(&f, "photo.jpg", b"jpeg-01", at);
    f.grant("alice", Rights::full());
    let s = f.open("photo.jpg");
    let a = f.join(&s, person("alice", "Alice"));
    save_in_place_at(&path, b"jpeg-02", at);
    f.advance(3_000);
    let rep = f.mgr.tick().unwrap();
    assert_eq!(rep.ingested.len(), 1, "{rep:?}");
    assert_eq!(read_all(&s, a), b"jpeg-02");
    assert_eq!(s.status().unwrap().current.author, EXTERNAL_PID);
}

/// The host's watcher (`file_changed`) always reads and hashes: no stat shortcut.
#[test]
fn racy_file_changed_always_hashes() {
    let f = fx("racy-fc");
    let at = racy_mtime();
    let path = racy_file(&f, "photo.jpg", b"jpeg-01", at);
    f.grant("alice", Rights::full());
    let s = f.open("photo.jpg");
    let a = f.join(&s, person("alice", "Alice"));
    save_in_place_at(&path, b"jpeg-02", at);
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(read_all(&s, a), b"jpeg-02");
}

/// (e) A stat read back from the store after a reopen: the kit doesn't keep a racy stat,
/// so a same-size save inside its tick while nobody had the doc open is found on open.
#[test]
fn racy_a_save_while_closed_is_found_on_reopen() {
    let f = fx("racy-reopen");
    let at = racy_mtime();
    let path = racy_file(&f, "a.bin", b"blob-01", at);
    f.grant("alice", Rights::full());
    {
        let s = f.open("a.bin");
        f.join(&s, person("alice", "Alice"));
    }
    f.mgr.release().unwrap();
    save_in_place_at(&path, b"blob-02", at);
    let h = f.host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            f.store.clone(),
            f.dir.0.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(f.store.clone(), f.host.clone(), Config::default())
        .with_blob_store(blobs);
    let s = mgr
        .open_blob_file(Arc::new(AnyFile), &key("a.bin"), &path, &f.ws)
        .unwrap();
    let st = s.status().unwrap();
    assert_eq!(st.version, 2, "the save while closed is a version");
    assert_eq!(st.current.author, EXTERNAL_PID);
}

/// (d) The conflict-copy cleanup: a copy edited, then edited back to its version's bytes
/// inside the same mtime tick, is the kit's copy again (and goes), not "edited" for good.
#[test]
fn racy_a_copy_edited_back_inside_the_tick_is_the_kits_again() {
    let f = fx("racy-gc");
    f.conflicts_in(Some("conflicts"));
    fs::write(f.path("x.bin"), b"base").unwrap();
    f.grant("alice", Rights::full());
    let s = f.open("x.bin");
    let a = f.join(&s, person("alice", "Alice"));
    let (copy, _, _) = conflict(&f, &s, a, b"alice's", b"desktop");
    let at = racy_mtime();
    save_in_place_at(&copy, b"ALICE's", at);
    f.advance(31 * DAY);
    assert!(
        f.mgr.tick().unwrap().conflict_copies_removed.is_empty(),
        "edited: kept"
    );
    save_in_place_at(&copy, b"alice's", at);
    f.advance(DAY);
    assert_eq!(
        f.mgr.tick().unwrap().conflict_copies_removed,
        [(key("x.bin"), copy.clone())],
        "edited back to its version: the kit's copy again"
    );
}
