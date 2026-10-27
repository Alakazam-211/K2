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
    AnyFile, AuditEvent, BlobMerge, BlobPack, BlobSession, Config, ConnId, DiskCheck, DiskState,
    Host, LeaseEnd, Need, Participant, ParticipantKind, Rights, Role, SessionError, SessionManager,
    Stay, TextFilePack, EXTERNAL_PID,
};
use k2_mpc_base::store::{BlobReason, DocKey, DocStore, StoreError};

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
        "report (conflict 2026-10-27 143001 from Alice).pdf"
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
