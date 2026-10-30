//! Tier 0 → text promotion (v0.9.2) as a host uses it: a file refused as text (mixed line
//! endings) lives as a blob doc, is fixed, and is promoted to a text doc with the same key,
//! keeping its history: text versions for what decodes, tier-0 history for the rest;
//! approvals and purges carried over; restore of an imported version; refusals that leave
//! the blob doc as it was; a restart.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AnyFile, ApprovalState, AuditEvent, BlobLogKind, BlobSession, ChangeKind, Config, ConnId, Host,
    LogItem, LogQuery, Need, NotReached, Pack, Participant, ParticipantKind, Promoted,
    PurgeRequest, PurgeScope, Rights, SessionError, SessionManager, Stay, TextFilePack,
    EXTERNAL_PID, IMPORT_PID,
};
use k2_mpc_base::store::{scan_file, DocKey, DocStore, StoreError, VersionReason};

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
        let _ = fs::remove_dir_all(&self.0);
    }
}

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

fn manager(store: &Arc<DocStore>, host: &Arc<TestHost>, blobs: &Arc<BlobStore>) -> SessionManager {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs.clone())
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-promote-{tag}-{}-{}",
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
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr = manager(&store, &host, &blobs);
    let fx = Fx {
        dir: TmpDir(p),
        ws,
        host,
        store,
        blobs,
        mgr,
    };
    fx.grant("alice", Rights::full().with_owner(true));
    fx.grant("bot", Rights::full());
    fx
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn md() -> Arc<dyn Pack> {
    Arc::new(TextFilePack::new("markdown", 4 << 20))
}

fn alice() -> Participant {
    Participant::new("alice", "Alice", "#123456", ParticipantKind::Person, "tab")
}

/// An assistant agent working for alice.
fn bot() -> Participant {
    Participant::new("bot", "Bot", "#654321", ParticipantKind::Agent, "cli").on_behalf_of("alice")
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
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
    fn open_blob(&self, name: &str) -> Arc<BlobSession> {
        self.mgr
            .open_blob_file(Arc::new(AnyFile), &key(name), &self.path(name), &self.ws)
            .unwrap()
    }
    fn promote(
        &self,
        name: &str,
    ) -> Result<
        (
            Arc<k2_mpc_base::session::Session>,
            k2_mpc_base::session::Promotion,
        ),
        SessionError,
    > {
        self.mgr
            .promote_blob_file(md(), &key(name), &self.path(name), &self.ws)
    }
    /// Every place a secret could linger: the store file and every blob file.
    fn find(&self, needle: &[u8]) -> usize {
        let mut n = self.store.scan_for(&[needle]).unwrap().len();
        let mut stack = vec![self.dir.0.join("blobs")];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    n += scan_file(&p, &[needle]).unwrap().len();
                }
            }
        }
        n
    }
}

fn join_blob(s: &BlobSession, p: Participant) -> ConnId {
    s.join(p, Stay::Held).unwrap().conn
}

fn read_tier0(t: &k2_mpc_base::session::Session, c: ConnId, v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    t.read_tier0_version(c, v)
        .unwrap()
        .reader
        .read_to_end(&mut out)
        .unwrap();
    out
}

fn text(t: &k2_mpc_base::session::Session, c: ConnId) -> String {
    t.read(c).unwrap().text_by_root[0].1.clone()
}

fn changes(t: &k2_mpc_base::session::Session, c: ConnId) -> Vec<(ChangeKind, String, Option<u64>)> {
    let mut out: Vec<_> = t
        .log(c, &LogQuery::default())
        .unwrap()
        .into_iter()
        .filter_map(|i| match i {
            LogItem::Change { info, .. } => Some((info.kind, info.author, info.of)),
            _ => None,
        })
        .collect();
    out.reverse(); // oldest first
    out
}

fn text_of(i: &Promoted) -> (u64, u64, Option<u64>, bool) {
    match i {
        Promoted::Text {
            version,
            seq,
            change,
            tier0_kept,
        } => (*version, *seq, *change, *tier0_kept),
        other => panic!("not text: {other:?}"),
    }
}

// ---------- the main path ----------

/// A text version as compared: (seq, at_ms, reason, name, authors).
type VersionRow = (u64, u64, VersionReason, Option<String>, Vec<String>);

#[test]
fn promote_after_a_mixed_endings_refusal_then_a_fix_keeps_history() {
    let fx = fx("main");
    let path = fx.path("notes.md");
    fs::write(&path, b"one\r\ntwo\n").unwrap();
    // refused as text: the host keeps it as a blob doc
    let e = fx
        .mgr
        .open_file(md(), &key("notes.md"), &path, &fx.ws)
        .err()
        .unwrap();
    assert!(matches!(e, SessionError::Declined(ref d) if d.code == "mixed_line_endings"));
    let s = fx.open_blob("notes.md");
    let a = join_blob(&s, alice());
    let b = join_blob(&s, bot());
    // a desktop app fixes the line endings: an outside save (v2)
    fx.advance(60_000);
    fs::write(&path, b"one\ntwo\n").unwrap();
    s.file_changed().unwrap();
    // alice writes (v3); the bot writes for her (v4, approved); alice writes a binary (v5)
    fx.advance(60_000);
    s.write_bytes(a, b"one\ntwo\nthree\n", 2).unwrap();
    fx.advance(60_000);
    s.write_bytes(b, b"one\n2\nthree\n", 3).unwrap();
    s.set_approval(a, 4, ApprovalState::Approved).unwrap();
    fx.advance(60_000);
    s.write_bytes(a, b"\0\x01binary", 4).unwrap();
    // and restores v3 (v6), which she named
    fx.advance(60_000);
    s.restore(a, 3).unwrap();
    s.name_version(a, 3, "draft").unwrap();

    // history before
    let before = s.log(a).unwrap();
    let kinds: Vec<BlobLogKind> = before.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            BlobLogKind::Import,
            BlobLogKind::External,
            BlobLogKind::Write,
            BlobLogKind::Write,
            BlobLogKind::Write,
            BlobLogKind::Restore
        ]
    );
    let blob_versions = s.versions(a).unwrap();
    let at: Vec<u64> = blob_versions.iter().map(|(_, v)| v.at_ms).collect();

    fx.advance(60_000);
    let (t, done) = fx.promote("notes.md").unwrap();

    // what each version became
    assert_eq!(done.key, key("notes.md"));
    assert_eq!(done.versions.len(), 6);
    assert_eq!(
        done.versions[0],
        (
            1,
            Promoted::NonText {
                code: "mixed_line_endings"
            }
        )
    );
    assert_eq!(text_of(&done.versions[1].1), (1, 1, Some(1), false));
    assert_eq!(text_of(&done.versions[2].1), (2, 2, Some(2), false));
    assert_eq!(text_of(&done.versions[3].1), (3, 3, Some(3), false));
    assert_eq!(done.versions[4], (5, Promoted::NonText { code: "binary" }));
    assert_eq!(text_of(&done.versions[5].1), (4, 4, Some(4), false));
    assert_eq!(done.changes, 4);
    assert_eq!(done.tier0_kept(), 2);
    // v2, v3 (= v6) and v4's bytes now live in the text history only: removed at once
    assert_eq!(done.blobs_removed, 3);
    assert_eq!(
        fx.blobs.stats().unwrap().blobs,
        2,
        "v1's and v5's blobs stay"
    );
    let mut conns = done.connections.clone();
    conns.sort();
    let mut want = vec![(a, "alice".to_owned()), (b, "bot".to_owned())];
    want.sort();
    assert_eq!(conns, want);
    assert!(fx.events().iter().any(|e| matches!(
        e,
        AuditEvent::Promoted { key: k, text_versions: 4, changes: 4, tier0_kept: 2, .. }
            if *k == key("notes.md")
    )));

    // the blob session is gone; the key is a text doc now
    assert!(matches!(s.read(a).err(), Some(SessionError::Unloaded)));
    let e = fx
        .mgr
        .open_blob(Arc::new(AnyFile), &key("notes.md"), None)
        .err()
        .unwrap();
    assert!(matches!(e, SessionError::PackMismatch { ref stored, .. } if stored == "markdown"));

    // the text doc: the current content, the file untouched
    let c = t.join(alice(), Stay::Held).unwrap().conn;
    assert_eq!(text(&t, c), "one\ntwo\nthree\n");
    assert_eq!(fs::read(&path).unwrap(), b"one\ntwo\nthree\n");
    assert!(!t.flush(c).unwrap().written, "the file already holds it");

    // history after: one text version per text blob version, at its time, by its author
    let versions = t.versions(c).unwrap();
    let got: Vec<VersionRow> = versions
        .iter()
        .map(|v| (v.seq, v.at_ms, v.reason, v.name.clone(), v.authors.clone()))
        .collect();
    assert_eq!(
        got,
        [
            (
                1,
                at[1],
                VersionReason::Import,
                None,
                vec![EXTERNAL_PID.to_owned()]
            ),
            (
                2,
                at[2],
                VersionReason::Idle,
                Some("draft".into()),
                vec!["alice".into()]
            ),
            (3, at[3], VersionReason::Idle, None, vec!["bot".into()]),
            (4, at[5], VersionReason::Idle, None, vec!["alice".into()]),
        ]
    );
    assert_eq!(t.show(c, 1).unwrap()[0].1, "one\ntwo\n");
    assert_eq!(t.show(c, 3).unwrap()[0].1, "one\n2\nthree\n");
    assert_eq!(
        changes(&t, c),
        [
            (ChangeKind::Import, EXTERNAL_PID.to_owned(), None),
            (ChangeKind::Edit, "alice".to_owned(), None),
            (ChangeKind::Edit, "bot".to_owned(), None),
            (ChangeKind::Restore, "alice".to_owned(), Some(2)),
        ]
    );
    // the bot's change keeps its person and its approval
    let log = t.log(c, &LogQuery::default()).unwrap();
    let bots: Vec<_> = log
        .iter()
        .filter_map(|i| match i {
            LogItem::Change { info, approval } if info.author == "bot" => {
                Some((info.at_ms, info.on_behalf_of.clone(), *approval))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        bots,
        [(
            at[3],
            Some("alice".to_owned()),
            Some(ApprovalState::Approved)
        )]
    );
    // blame names each line's change, author and time from the tier-0 history
    let blame: Vec<(String, String, Option<u64>, Option<u64>)> = t
        .blame(c, "source", None)
        .unwrap()
        .lines
        .into_iter()
        .map(|l| (l.text, l.author, l.change, l.at_ms))
        .collect();
    assert_eq!(
        blame,
        [
            ("one".into(), EXTERNAL_PID.into(), Some(1), Some(at[1])),
            ("two".into(), "alice".into(), Some(4), Some(at[5])),
            ("three".into(), "alice".into(), Some(2), Some(at[2])),
        ]
    );
    let ap = t.approval(c, 3).unwrap().unwrap();
    assert_eq!(ap.state, ApprovalState::Approved);
    assert_eq!(ap.by.as_deref(), Some("alice"));

    // the tier-0 history: what didn't read as text, bytes intact
    let tier0 = t.tier0_log(c).unwrap();
    let ids: Vec<(u64, BlobLogKind, String)> = tier0
        .iter()
        .map(|e| (e.id, e.kind, e.author.clone()))
        .collect();
    assert_eq!(
        ids,
        [
            (1, BlobLogKind::Import, IMPORT_PID.to_owned()),
            (5, BlobLogKind::Write, "alice".to_owned())
        ]
    );
    assert_eq!(tier0[0].at_ms, before[0].at_ms);
    assert_eq!(read_tier0(&t, c, 1), b"one\r\ntwo\n");
    assert_eq!(read_tier0(&t, c, 5), b"\0\x01binary");
    assert!(matches!(
        t.read_tier0_version(c, 3).err(),
        Some(SessionError::Store(StoreError::NoVersion(_, 3)))
    ));

    // restore of an imported version: a new change by the restorer, written to the file
    fx.advance(1_000);
    t.restore(c, 1).unwrap();
    assert_eq!(text(&t, c), "one\ntwo\n");
    t.flush(c).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"one\ntwo\n");
    let last = changes(&t, c).pop().unwrap();
    assert_eq!(last, (ChangeKind::Restore, "alice".to_owned(), Some(1)));

    // a restart: the same text doc, its history and its tier-0 history
    drop(t);
    fx.mgr.release().unwrap();
    let mgr = manager(&fx.store, &fx.host, &fx.blobs);
    let t = mgr
        .open_file(md(), &key("notes.md"), &path, &fx.ws)
        .unwrap();
    let c = t.join(alice(), Stay::Held).unwrap().conn;
    assert_eq!(text(&t, c), "one\ntwo\n");
    assert_eq!(changes(&t, c).len(), 5);
    assert_eq!(t.tier0_log(c).unwrap().len(), 2);
    assert_eq!(read_tier0(&t, c, 5), b"\0\x01binary");
    drop(t);
    mgr.release().unwrap();
}

#[test]
fn an_on_behalf_version_without_a_text_change_keeps_its_approval_in_tier0() {
    let fx = fx("approval");
    let path = fx.path("a.md");
    fs::write(&path, b"one\ntwo\n").unwrap();
    let s = fx.open_blob("a.md");
    let a = join_blob(&s, alice());
    let b = join_blob(&s, bot());
    fx.advance(1_000);
    s.write_bytes(b, b"\0bin", 1).unwrap(); // v2: pending, not text
    fx.advance(1_000);
    s.write_bytes(a, b"one\ntwo\nthree\n", 2).unwrap(); // v3
    fx.advance(1_000);
    s.write_bytes(b, b"one\r\ntwo\r\nthree\r\n", 3).unwrap(); // v4: pending, same text
    fx.advance(1_000);
    let (t, done) = fx.promote("a.md").unwrap();
    assert_eq!(text_of(&done.versions[0].1), (1, 1, Some(1), false));
    assert_eq!(done.versions[1], (2, Promoted::NonText { code: "binary" }));
    assert_eq!(text_of(&done.versions[2].1), (2, 2, Some(2), false));
    assert_eq!(
        text_of(&done.versions[3].1),
        (3, 2, None, true),
        "no change to carry its approval: it stays in tier 0 too"
    );
    let c = t.join(alice(), Stay::Held).unwrap().conn;
    let cb = t.join(bot(), Stay::Held).unwrap().conn;
    // the file's format is the current version's (CRLF)
    assert_eq!(t.read(c).unwrap().bytes, b"one\r\ntwo\r\nthree\r\n");
    let tier0: Vec<(u64, Option<ApprovalState>)> = t
        .tier0_log(c)
        .unwrap()
        .iter()
        .map(|e| (e.id, e.approval))
        .collect();
    assert_eq!(
        tier0,
        [
            (2, Some(ApprovalState::Pending)),
            (4, Some(ApprovalState::Pending))
        ]
    );
    // only the person settles it, once
    assert!(matches!(
        t.set_tier0_approval(cb, 2, ApprovalState::Approved).err(),
        Some(SessionError::Forbidden {
            need: Need::Approver
        })
    ));
    let got = t.set_tier0_approval(c, 2, ApprovalState::Declined).unwrap();
    assert_eq!(got.approval.state, ApprovalState::Declined);
    assert_eq!(got.restore_to, None);
    assert_eq!(got.later, [4]);
    assert_eq!(
        t.set_tier0_approval(c, 2, ApprovalState::Declined)
            .unwrap()
            .approval,
        got.approval
    );
    assert!(matches!(
        t.set_tier0_approval(c, 2, ApprovalState::Approved).err(),
        Some(SessionError::BadApproval(_))
    ));
    assert!(matches!(
        t.set_tier0_approval(c, 3, ApprovalState::Approved).err(),
        Some(SessionError::NoApproval { seq: 3 })
    ));
    // the text history has no approval for them (no change was made on her behalf)
    assert!(t.approvals(c).unwrap().is_empty());
    drop(t);
}

// ---------- refusals leave the blob doc as it was ----------

#[test]
fn promotion_refusals_change_nothing() {
    let fx = fx("refuse");
    let path = fx.path("r.md");
    fs::write(&path, b"text\n").unwrap();
    let s = fx.open_blob("r.md");
    let a = join_blob(&s, alice());
    fx.advance(1_000);
    s.write_bytes(a, b"\0binary now", 1).unwrap();
    // the current version isn't text
    let e = fx.promote("r.md").err().unwrap();
    assert!(
        matches!(e, SessionError::Declined(ref d) if d.code == "binary"),
        "{e:?}"
    );
    assert_eq!(s.versions(a).unwrap().len(), 2, "still a blob doc");
    // someone holds the lease
    fx.advance(1_000);
    s.write_bytes(a, b"text again\n", 2).unwrap();
    s.acquire_lease(a, None).unwrap();
    let e = fx.promote("r.md").err().unwrap();
    assert!(
        matches!(e, SessionError::Leased { ref holder, .. } if holder == "alice"),
        "{e:?}"
    );
    assert_eq!(s.versions(a).unwrap().len(), 3);
    s.release_lease(a).unwrap();
    // not a blob doc, or no doc
    fs::write(fx.path("t.md"), b"x\n").unwrap();
    fx.mgr
        .open_file(md(), &key("t.md"), &fx.path("t.md"), &fx.ws)
        .unwrap();
    assert!(matches!(
        fx.promote("t.md").err(),
        Some(SessionError::PackMismatch { .. })
    ));
    assert!(matches!(
        fx.mgr.promote_blob(md(), &key("none.md")).err(),
        Some(SessionError::NotFound(_))
    ));
    // and then it goes through
    let (t, done) = fx.promote("r.md").unwrap();
    assert_eq!(done.changes, 2);
    let c = t.join(alice(), Stay::Held).unwrap().conn;
    assert_eq!(text(&t, c), "text again\n");
}

#[test]
fn a_blob_doc_without_a_file_promotes_too() {
    let fx = fx("nofile");
    let s = fx
        .mgr
        .open_blob(Arc::new(AnyFile), &key("m.md"), Some(&mut &b"a\r\nb\n"[..]))
        .unwrap();
    let a = join_blob(&s, alice());
    fx.advance(1_000);
    s.write_bytes(a, b"a\nb\n", 1).unwrap();
    let (t, done) = fx.mgr.promote_blob(md(), &key("m.md")).unwrap();
    assert_eq!(done.changes, 1);
    let c = t.join(alice(), Stay::Held).unwrap().conn;
    assert_eq!(text(&t, c), "a\nb\n");
    assert_eq!(t.tier0_log(c).unwrap().len(), 1);
}

// ---------- purge ----------

#[test]
fn purge_carries_over_and_reaches_the_tier0_history() {
    let fx = fx("purge");
    let path = fx.path("p.md");
    fs::write(&path, b"token SECRET-AAAA-1111\n").unwrap();
    let s = fx.open_blob("p.md");
    let a = join_blob(&s, alice());
    fx.advance(1_000);
    s.write_bytes(a, b"clean\n", 1).unwrap(); // v2
    s.purge(a, Some(1)).unwrap(); // v1 purged before the promotion
    fx.advance(1_000);
    s.write_bytes(a, b"pw SECRET-BBBB-2222\r\nx\n", 2).unwrap(); // v3: mixed endings
    fx.advance(1_000);
    s.write_bytes(a, b"clean\nmore\n", 3).unwrap(); // v4
    assert_eq!(fx.find(b"SECRET-AAAA-1111"), 0);
    fx.advance(1_000);
    let (t, done) = fx.promote("p.md").unwrap();
    assert_eq!(done.versions[0], (1, Promoted::Purged));
    assert_eq!(text_of(&done.versions[1].1), (1, 1, Some(1), false));
    assert_eq!(
        done.versions[2],
        (
            3,
            Promoted::NonText {
                code: "mixed_line_endings"
            }
        )
    );
    assert_eq!(text_of(&done.versions[3].1), (2, 2, Some(2), false));
    let c = t.join(alice(), Stay::Held).unwrap().conn;
    // nothing purged came back
    assert_eq!(fx.find(b"SECRET-AAAA-1111"), 0);
    for v in t.versions(c).unwrap() {
        assert!(!t.show(c, v.id).unwrap()[0].1.contains("SECRET"));
    }
    let tier0 = t.tier0_log(c).unwrap();
    assert_eq!(
        tier0.iter().map(|e| (e.id, e.redacted)).collect::<Vec<_>>(),
        [(1, true), (3, false)]
    );
    assert!(matches!(
        t.read_tier0_version(c, 1).err(),
        Some(SessionError::Store(StoreError::PurgedVersion(_, 1)))
    ));

    // a text purge doesn't search the tier-0 history, and says so
    assert!(fx.find(b"SECRET-BBBB-2222") > 0);
    let out = t
        .purge(
            c,
            PurgeRequest::new(PurgeScope::Text("SECRET-BBBB-2222".into())),
        )
        .unwrap();
    assert!(out.not_reached.contains(&NotReached::Tier0History(1)));
    // purge_tier0 does (owner only)
    let cb = t.join(bot(), Stay::Held).unwrap().conn;
    assert!(matches!(
        t.purge_tier0(cb, Some(3)).err(),
        Some(SessionError::Forbidden { need: Need::Owner })
    ));
    let out = t.purge_tier0(c, Some(3)).unwrap();
    assert_eq!(out.counts.blob_versions, 1);
    assert_eq!(out.counts.blobs_removed, 1);
    assert_eq!(fx.find(b"SECRET-BBBB-2222"), 0);
    assert!(t.tier0_log(c).unwrap().iter().all(|e| e.redacted));
    assert_eq!(text(&t, c), "clean\nmore\n", "live text untouched");

    // on another doc, a History purge reaches its tier-0 history
    let path2 = fx.path("q.md");
    fs::write(&path2, b"pw SECRET-CCCC-3333\r\nx\n").unwrap();
    let s2 = fx.open_blob("q.md");
    let a2 = join_blob(&s2, alice());
    fx.advance(1_000);
    s2.write_bytes(a2, b"x\n", 1).unwrap();
    let (t2, done) = fx.promote("q.md").unwrap();
    assert_eq!(done.tier0_kept(), 1);
    let c2 = t2.join(alice(), Stay::Held).unwrap().conn;
    assert!(fx.find(b"SECRET-CCCC-3333") > 0);
    let out = t2
        .purge(c2, PurgeRequest::new(PurgeScope::History))
        .unwrap();
    assert_eq!(out.counts.blob_versions, 1);
    assert_eq!(out.counts.blobs_removed, 1);
    assert!(!out
        .not_reached
        .iter()
        .any(|n| matches!(n, NotReached::Tier0History(_))));
    assert_eq!(fx.find(b"SECRET-CCCC-3333"), 0);
    assert!(t2.tier0_log(c2).unwrap()[0].redacted);
    assert_eq!(text(&t2, c2), "x\n");
}

#[test]
fn deleting_a_promoted_doc_releases_its_tier0_blobs() {
    let fx = fx("delete");
    let path = fx.path("d.md");
    fs::write(&path, b"\0bin").unwrap();
    let s = fx.open_blob("d.md");
    let a = join_blob(&s, alice());
    fx.advance(1_000);
    s.write_bytes(a, b"now text\n", 1).unwrap();
    let (t, _) = fx.promote("d.md").unwrap();
    drop(t);
    assert_eq!(fx.blobs.stats().unwrap().blobs, 1);
    assert_eq!(fx.blobs.stats().unwrap().unreferenced_bytes, 0);
    fx.mgr.delete_doc(&key("d.md")).unwrap();
    assert_eq!(fx.blobs.stats().unwrap().unreferenced_bytes, 4);
    let _ = Path::new(&fx.dir.0);
}

// ---------- D1 (v0.9.7) ----------

/// The D1 amendment's class (a) through a promotion: p16 types `p16:3027` and changes its
/// `7` to a `9`, p14 replaces the line with `p14:3169`, all as tier-0 versions. Promotion
/// replays each version as a change by its author, widened as any diff-based change (undo
/// spec v1.10, D1a): p14's change rewrites the whole token, so p16's revert of its `7`→`9`
/// conflicts and p14's line stays. Returns the revert's outcome, the text after it and
/// whether the store kept an authored row for p14's change (it does only when D1 widened it).
fn promoted_class_a() -> (k2_mpc_base::session::UndoOutcome, String, bool) {
    let fx = fx("d1");
    let agent = |pid: &str| Participant::new(pid, pid, "#336699", ParticipantKind::Agent, "cli");
    fx.grant("p16", Rights::full());
    fx.grant("p14", Rights::full());
    let s = fx
        .mgr
        .open_blob(Arc::new(AnyFile), &key("d1.md"), Some(&mut &b"seed\n"[..]))
        .unwrap();
    let p16 = join_blob(&s, agent("p16"));
    let p14 = join_blob(&s, agent("p14"));
    fx.advance(1_000);
    s.write_bytes(p16, b"seed\np16:3027\n", 1).unwrap();
    fx.advance(1_000);
    s.write_bytes(p16, b"seed\np16:3029\n", 2).unwrap();
    fx.advance(1_000);
    s.write_bytes(p14, b"seed\np14:3169\n", 3).unwrap();
    let (t, done) = fx.mgr.promote_blob(md(), &key("d1.md")).unwrap();
    assert_eq!(done.changes, 4);
    let change = |vid: u64| {
        let (_, _, c, _) = text_of(&done.versions.iter().find(|(v, _)| *v == vid).unwrap().1);
        c.expect("a change")
    };
    let (nine, p14s) = (change(3), change(4));
    let id = fx.store.find(&key("d1.md")).unwrap().unwrap();
    let row = fx.store.authored_hunks(id, p14s).unwrap();
    let c16 = t.join(agent("p16"), Stay::Held).unwrap().conn;
    let out = t.revert(c16, nine, false).unwrap();
    let after = text(&t, c16);
    if let Some(row) = &row {
        // the authored row holds the diff's hunks, inside the token the record took whole
        assert_eq!(row.len(), 1);
        assert!(row[0].1[0].start > 5, "{row:?}");
    }
    (out, after, row.is_some())
}

#[test]
fn promotion_widens_each_version_so_a_revert_never_changes_another_authors_word() {
    use k2_mpc_base::differential::{with_d1_mutant, D1Mutant};
    use k2_mpc_base::session::UndoOutcome;
    let (out, after, widened) = promoted_class_a();
    assert!(widened, "D1 widened p14's version");
    assert!(
        matches!(out, UndoOutcome::Conflicts { .. }),
        "p16's revert conflicts: {out:?}"
    );
    assert_eq!(after, "seed\np14:3169\n", "p14's line is untouched");
    // the negative control: without D1 the revert changes p14's word
    let (out, after, widened) = with_d1_mutant(D1Mutant::ReuseForeign, promoted_class_a);
    assert!(!widened);
    assert!(
        !matches!(out, UndoOutcome::Conflicts { .. }) && after != "seed\np14:3169\n",
        "without D1: {out:?}, {after:?}"
    );
}

/// A promoted version that deleted a line, reverted after promotion: the revert puts back the
/// deleted line's roots (undo spec U23; fixed in v0.9.7.2: the promoted change's record kept
/// no roots, so its revert put back fresh text). Then the revert undone and redone.
fn promoted_deletion_reverted() -> Result<(), String> {
    let fx = fx("prov");
    let agent = |pid: &str| Participant::new(pid, pid, "#336699", ParticipantKind::Agent, "cli");
    fx.grant("p16", Rights::full());
    let s = fx
        .mgr
        .open_blob(Arc::new(AnyFile), &key("pr.md"), Some(&mut &b"a\nc\n"[..]))
        .unwrap();
    let p16 = join_blob(&s, agent("p16"));
    fx.advance(1_000);
    s.write_bytes(p16, b"a\nb\nc\n", 1).unwrap();
    fx.advance(1_000);
    s.write_bytes(p16, b"a\nc\n", 2).unwrap();
    let (t, done) = fx.mgr.promote_blob(md(), &key("pr.md")).unwrap();
    let change = |vid: u64| {
        let (_, _, c, _) = text_of(&done.versions.iter().find(|(v, _)| *v == vid).unwrap().1);
        c.expect("a change")
    };
    let (added, deleted) = (change(2), change(3));
    let c = t.join(alice(), Stay::Held).unwrap().conn;
    let b_roots = |t: &k2_mpc_base::session::Session| -> Vec<Option<u64>> {
        t.char_blame(TextFilePack::ROOT)
            .unwrap()
            .iter()
            .filter(|x| x.ch == 'b')
            .map(|x| x.root_change)
            .collect()
    };
    let restored_by = |t: &k2_mpc_base::session::Session| {
        t.blame(c, TextFilePack::ROOT, None)
            .unwrap()
            .lines
            .iter()
            .find(|l| l.text == "b")
            .and_then(|l| l.restored_by.clone())
    };
    for partial in [false, true] {
        let out = t.revert(c, deleted, partial).unwrap();
        if text(&t, c) != "a\nb\nc\n" {
            return Err(format!("partial {partial}: {out:?}, {:?}", text(&t, c)));
        }
        if b_roots(&t) != vec![Some(added)] {
            return Err(format!(
                "partial {partial}: b's root change {:?}, not {added} (U23)",
                b_roots(&t)
            ));
        }
        if restored_by(&t).as_deref() != Some("alice") {
            return Err(format!(
                "partial {partial}: blame's restored_by {:?}",
                restored_by(&t)
            ));
        }
        t.undo(c, 1).unwrap();
        if text(&t, c) != "a\nc\n" {
            return Err(format!(
                "partial {partial}: undo of the revert: {:?}",
                text(&t, c)
            ));
        }
        t.redo(c, 1).unwrap();
        if b_roots(&t) != vec![Some(added)] || restored_by(&t).as_deref() != Some("alice") {
            return Err(format!(
                "partial {partial}: after the redo: {:?}",
                b_roots(&t)
            ));
        }
        t.undo(c, 1).unwrap();
    }
    Ok(())
}

#[test]
fn a_revert_of_a_promoted_deletion_puts_back_its_roots() {
    if let Err(e) = promoted_deletion_reverted() {
        panic!("{e}");
    }
}

/// The planted fault (`ProvMutant::OutsideSaveKeepsNoRoots`, which also drops a promoted
/// change's roots) fails it.
#[test]
fn a_promoted_deletion_without_roots_is_caught() {
    use k2_mpc_base::differential::{with_prov_mutant, ProvMutant};
    let r = with_prov_mutant(
        ProvMutant::OutsideSaveKeepsNoRoots,
        promoted_deletion_reverted,
    );
    assert!(r.is_err(), "the planted fault went unseen");
}
