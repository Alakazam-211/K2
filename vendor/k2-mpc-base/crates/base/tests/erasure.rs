//! Erasure (K-PURGE-1…13, K-T0-9, undo spec U19/U20) as a host uses it, on real temp folders
//! with the host's injected clock: purge leaves no byte of a secret in the store file (by
//! whole copy or any 8-byte window), its file or a conflict copy; undo stacks are cleared;
//! a revert over purged text reports `erased`; keyed sync hashes rotate; participant
//! erasure; tier-0 per-version purge. Every scan has its negative control: the same scan
//! finds the secret before the purge.
//!
//! **Size.** `MPC_ERASURE_SCALE` (default 1) multiplies the filler in
//! `purge_at_scale_leaves_no_bytes_behind`: CI size here, full size on a compute node:
//!
//! ```text
//! k2 compute run k2n7-vin --sha <sha> --needs rust@stable,cmake@3 --mem 24G -- \
//!   env MPC_ERASURE_SCALE=200 cargo test -p k2-mpc-base --release --test erasure
//! ```

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    Action, AnyFile, AuditEvent, BlobSession, Config, ConflictKind, ConnId, DiskCheck, Host, Need,
    NotReached, Pack, Participant, ParticipantKind, PurgeRequest, PurgeScope, PurgedScope, Rights,
    Session, SessionError, SessionManager, Stay, TextFilePack, UndoOutcome, WriteOutcome,
};
use k2_mpc_base::store::{scan_file, DocKey, DocStore, Finding, StoreError};
use k2_mpc_base::Doc;

const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;

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
    _dir: TmpDir,
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

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-erase-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_111_400_000),
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
    let mgr = SessionManager::new(store.clone(), host.clone(), config()).with_blob_store(blobs);
    let f = Fx {
        _dir: TmpDir(p),
        ws,
        host,
        store,
        mgr,
    };
    f.grant("own", Rights::full().with_owner(true));
    f.grant("ed", Rights::full());
    f.grant("bob-the-agent-0042", Rights::editor());
    f
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
    /// A text doc synced with `name`, created with `text`.
    fn open(&self, name: &str, text: &str) -> Arc<Session> {
        let p = self.path(name);
        if !p.exists() {
            fs::write(&p, text).unwrap();
        }
        self.mgr
            .open_file(Arc::new(MD), &key(name), &p, &self.ws)
            .unwrap()
    }
    fn id(&self, name: &str) -> u64 {
        self.store.find(&key(name)).unwrap().unwrap()
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
}

fn join(s: &Session, p: Participant) -> ConnId {
    s.join(p, Stay::Held).unwrap().conn
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root.remove(0).1
}

/// A random 32-byte secret, as 64 hex characters (gate tests use random secrets).
fn secret() -> String {
    let b: [u8; 32] = rand::random();
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Which of `needles` the store file holds (whole or by any 8-byte window).
fn in_store(store: &DocStore, needles: &[&str]) -> Vec<usize> {
    let b: Vec<&[u8]> = needles.iter().map(|s| s.as_bytes()).collect();
    hit_needles(&store.scan_for(&b).unwrap())
}

fn in_file(p: &Path, needles: &[&str]) -> Vec<usize> {
    let b: Vec<&[u8]> = needles.iter().map(|s| s.as_bytes()).collect();
    hit_needles(&scan_file(p, &b).unwrap())
}

fn hit_needles(f: &[Finding]) -> Vec<usize> {
    let mut v: Vec<usize> = f.iter().map(|f| f.needle).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn purge(s: &Session, c: ConnId, scope: PurgeScope) -> k2_mpc_base::session::PurgeOutcome {
    s.purge(c, PurgeRequest::new(scope)).unwrap()
}

// ---------- the gate: no bytes behind ----------

/// K-PURGE-3 `purge::byte_scan`: secrets planted in every place the kit keeps content (the
/// log, versions, revert records, an undo stack, the read-base cache, the sync replica and
/// the file, an outside edit), one of them typed in two halves (two records); a history
/// purge; the store file, the doc's file and the live text then hold none of them. Before
/// the purge the same scan finds every one (the negative control).
#[test]
fn history_purge_leaves_no_bytes_behind() {
    let f = fx("gate");
    let s = f.open("notes.md", "# notes\n\nkeep this line\n");
    let own = join(&s, person("own"));
    let ed = join(&s, person("ed"));
    let [s1, s2, s3, s4, s5, s6] = [secret(), secret(), secret(), secret(), secret(), secret()];
    let base = text(&s, ed);
    // s1: pasted, read (a read base), then deleted: log, revert records, undo stack
    s.replace_text(ed, ROOT, &format!("{base}token {s1}\n"))
        .unwrap();
    s.read(ed).unwrap();
    s.replace_text(ed, ROOT, &base).unwrap();
    // s2: typed in two halves (two changes), then deleted
    let (h1, h2) = s2.split_at(32);
    s.replace_text(ed, ROOT, &format!("{base}{h1}")).unwrap();
    s.replace_text(ed, ROOT, &format!("{base}{h1}{h2}\n"))
        .unwrap();
    s.replace_text(ed, ROOT, &base).unwrap();
    // s3: only in a named version (and history) once deleted
    s.replace_text(ed, ROOT, &format!("{base}v: {s3}\n"))
        .unwrap();
    s.name_version(ed, "with s3", None).unwrap();
    s.replace_text(ed, ROOT, &base).unwrap();
    // s4: written to the file (the sync replica holds it), deleted, not yet written back
    s.replace_text(ed, ROOT, &format!("{base}file: {s4}\n"))
        .unwrap();
    s.flush(ed).unwrap();
    s.replace_text(ed, ROOT, &base).unwrap();
    // s5: an outside edit adds it; then it is deleted in the doc
    fs::write(f.path("notes.md"), format!("{base}file: {s4}\nout: {s5}\n")).unwrap();
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    s.replace_text(ed, ROOT, &base).unwrap();
    // s6: typed and undone (the redo stack holds it)
    s.replace_text(ed, ROOT, &format!("{base}u: {s6}\n"))
        .unwrap();
    assert!(matches!(
        s.undo(ed, 1).unwrap(),
        UndoOutcome::Applied { .. }
    ));
    let all = [&*s1, &*s2, &*s3, &*s4, &*s5, &*s6];
    assert_eq!(text(&s, ed), base);
    // the negative control: every secret is in the store file, s4 in the file too
    assert_eq!(in_store(&f.store, &all), vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(in_file(&f.path("notes.md"), &all), vec![3, 4]);

    let out = purge(&s, own, PurgeScope::History);
    assert_eq!(out.live_matches, 0);
    assert!(out.counts.log_entries > 0 && out.counts.revert_hunks > 0);
    assert!(out.counts.versions > 0 && out.counts.undo_batches > 0);
    assert!(out.counts.read_bases > 0);
    assert!(out.rewrite.is_some());
    for n in NotReached::ALWAYS {
        assert!(out.not_reached.contains(&n), "{n:?} listed");
    }
    assert!(
        !out.not_reached.contains(&NotReached::File),
        "the file was rewritten"
    );

    assert_eq!(in_store(&f.store, &all), Vec::<usize>::new(), "store file");
    assert_eq!(
        in_file(&f.path("notes.md"), &all),
        Vec::<usize>::new(),
        "the file"
    );
    // live text and what's kept is untouched; the kept line is still in the store
    assert_eq!(text(&s, ed), base);
    assert_eq!(in_store(&f.store, &["keep this line"]), vec![0]);
    // undo and redo: cleared (U19), said once
    assert_eq!(s.redo(ed, 1).unwrap(), UndoOutcome::ClearedByPurge);
    assert_eq!(s.undo(ed, 1).unwrap(), UndoOutcome::Nothing);
    // versions keep their entries, named ones their names, without content
    let vs = s.versions(own).unwrap();
    let named = vs
        .iter()
        .find(|v| v.name.as_deref() == Some("with s3"))
        .unwrap();
    assert!(!named.redacted.is_empty());
    assert!(!s.show(own, named.id).unwrap()[0].1.contains(&s3[..8]));
}

/// Without the rewrite, a deleted row's bytes stay in the store file: what the rewrite is
/// for. (A redb page freed by a delete is not overwritten.)
#[test]
fn a_delete_alone_leaves_bytes_and_the_rewrite_removes_them() {
    let f = fx("residue");
    let sec = secret();
    f.store.host_put("t", "k", sec.as_bytes()).unwrap();
    assert!(f.store.host_delete("t", "k").unwrap());
    assert_eq!(
        in_store(&f.store, &[&sec]),
        vec![0],
        "freed pages keep the deleted bytes"
    );
    let r = f.store.rewrite_file().unwrap();
    assert!(r.rows > 0);
    assert_eq!(in_store(&f.store, &[&sec]), Vec::<usize>::new());
    // the store still works after the swap, and across a reopen
    f.store.host_put("t", "k2", b"v").unwrap();
    assert_eq!(
        f.store.host_get("t", "k2").unwrap().as_deref(),
        Some(&b"v"[..])
    );
}

/// A purge, then a reload from the store and a restart: the text and history survive;
/// change ids and lengths survive (K-PURGE-5): a replica synced before the purge still
/// merges into the doc after it.
#[test]
fn reload_and_convergence_after_purge() {
    let f = fx("reload");
    let s = f.open("a.md", "one\ntwo\n");
    let own = join(&s, person("own"));
    let ed = join(&s, person("ed"));
    let sec = secret();
    s.replace_text(ed, ROOT, &format!("one\n{sec}\ntwo\n"))
        .unwrap();
    s.replace_text(ed, ROOT, "one\ntwo\nthree\n").unwrap();
    let id = f.id("a.md");
    // a client replica, synced before the purge
    let mut client = Doc::from_store(MD.schema(), &f.store.load(id).unwrap()).unwrap();
    let log_before = s.log(own, &Default::default()).unwrap().len();
    purge(&s, own, PurgeScope::History);
    assert_eq!(in_store(&f.store, &[&sec]), Vec::<usize>::new());
    // the log keeps every change (ids, authors, times)
    assert_eq!(s.log(own, &Default::default()).unwrap().len(), log_before);
    // edits continue, and the client's edit made on pre-purge ids merges
    s.replace_text(ed, ROOT, "one\ntwo\nthree\nfour\n").unwrap();
    let u = client
        .replace_text_as(77, ROOT, "zero\none\ntwo\nthree\n")
        .unwrap()
        .unwrap();
    let mut server = Doc::from_store(MD.schema(), &f.store.load(id).unwrap()).unwrap();
    server.apply_client_update(&u, 77).unwrap();
    assert_eq!(server.text(ROOT).unwrap(), "zero\none\ntwo\nthree\nfour\n");
    // restart: a new manager reads the purged store
    drop(s);
    f.mgr.release().unwrap();
    let mgr = SessionManager::new(f.store.clone(), f.host.clone(), config());
    let s = mgr
        .open_file(Arc::new(MD), &key("a.md"), &f.path("a.md"), &f.ws)
        .unwrap();
    let ed = join(&s, person("ed"));
    assert_eq!(text(&s, ed), "one\ntwo\nthree\nfour\n");
    s.replace_text(ed, ROOT, "one\ntwo\nthree\nfour\nfive\n")
        .unwrap();
    s.flush(ed).unwrap();
    assert_eq!(
        fs::read_to_string(f.path("a.md")).unwrap(),
        "one\ntwo\nthree\nfour\nfive\n"
    );
    assert_eq!(in_store(&f.store, &[&sec]), Vec::<usize>::new());
    mgr.release().unwrap();
}

// ---------- scopes ----------

/// `Changes`: the targeted change's text goes from everywhere (its record, a version, the
/// record of the change that deleted it); unrelated history stays.
#[test]
fn purge_changes_reaches_copies_and_spares_the_rest() {
    let f = fx("changes");
    let s = f.open("a.md", "a\n");
    let own = join(&s, person("own"));
    let ed = join(&s, person("ed"));
    let (sec, other) = (secret(), secret());
    let pasted = s
        .replace_text(ed, ROOT, &format!("a\n{sec}\n"))
        .unwrap()
        .version;
    s.name_version(ed, "v", None).unwrap();
    s.replace_text(ed, ROOT, &format!("a\n{other}\n")).unwrap(); // deletes sec, adds other
    s.replace_text(ed, ROOT, "a\n").unwrap(); // deletes other
    assert_eq!(in_store(&f.store, &[&sec, &other]), vec![0, 1]);
    let out = purge(
        &s,
        own,
        PurgeScope::Changes {
            from: pasted,
            to: pasted,
        },
    );
    assert_eq!(out.live_matches, 0);
    assert_eq!(in_store(&f.store, &[&sec]), Vec::<usize>::new());
    // the other secret's records weren't targeted and don't hold the purged text
    assert_eq!(
        in_store(&f.store, &[&other]),
        vec![0],
        "untargeted history stays"
    );
    // the version that held it keeps its entry, without the span
    let v = s
        .versions(own)
        .unwrap()
        .into_iter()
        .find(|v| v.name.as_deref() == Some("v"))
        .unwrap();
    // the change inserted the secret and its line break: that span goes
    assert_eq!(v.redacted.len(), 1);
    assert_eq!(
        (v.redacted[0].start, v.redacted[0].len),
        (2, sec.len() as u64 + 1)
    );
    assert_eq!(s.show(own, v.id).unwrap()[0].1, "a\n");
}

/// `Text`, with and without `and_remove`. Without it, live text is left alone and the
/// outcome says the text is still live (and the store still holds it, in the live state).
#[test]
fn purge_text_and_and_remove() {
    let f = fx("text");
    let s = f.open("a.md", "a\n");
    let own = join(&s, person("own"));
    let ed = join(&s, person("ed"));
    let sec = secret();
    s.replace_text(ed, ROOT, &format!("a {sec}\nb {sec}\n"))
        .unwrap();
    let out = purge(&s, own, PurgeScope::Text(sec.clone()));
    assert_eq!(out.live_matches, 2, "still live: purge doesn't edit");
    assert_eq!(text(&s, ed), format!("a {sec}\nb {sec}\n"));
    assert_eq!(
        in_store(&f.store, &[&sec]),
        vec![0],
        "the live state holds it"
    );
    // and_remove: an ordinary change by the owner deletes every copy, then the purge
    let out = s
        .purge(
            own,
            PurgeRequest::new(PurgeScope::Text(sec.clone())).and_remove(),
        )
        .unwrap();
    assert!(out.removed.is_some());
    assert_eq!(out.live_matches, 0);
    assert_eq!(text(&s, ed), "a \nb \n");
    assert_eq!(in_store(&f.store, &[&sec]), Vec::<usize>::new());
    assert_eq!(in_file(&f.path("a.md"), &[&sec]), Vec::<usize>::new());
    // and_remove needs a text scope; an empty text is refused
    assert!(matches!(
        s.purge(own, PurgeRequest::new(PurgeScope::History).and_remove()),
        Err(SessionError::BadEdit(_))
    ));
    assert!(matches!(
        s.purge(own, PurgeRequest::new(PurgeScope::Text(String::new()))),
        Err(SessionError::BadEdit(_))
    ));
}

/// A text under 8 bytes would match innocent text across the whole history: refused with a
/// usage error (exit 2) and a hint, and nothing is purged (the control: an 8-byte text is
/// accepted).
#[test]
fn a_purge_text_under_8_bytes_is_refused() {
    let f = fx("short");
    let s = f.open(
        "a.md", "a
",
    );
    let own = join(&s, person("own"));
    let ed = join(&s, person("ed"));
    s.replace_text(ed, ROOT, "pin 4821 and innocent words\n")
        .unwrap();
    s.replace_text(ed, ROOT, "a\n").unwrap();
    for t in ["4821", "1234567"] {
        let e = s
            .purge(own, PurgeRequest::new(PurgeScope::Text(t.into())))
            .unwrap_err();
        assert_eq!(k2_mpc_base::tools::session_exit(&e), 2);
        match &e {
            SessionError::BadEdit(m) => {
                assert!(m.contains("at least 8 bytes"), "{m}");
                assert!(m.contains("change/version/participant"), "{m}");
            }
            e => panic!("expected BadEdit, got {e:?}"),
        }
    }
    assert!(!f
        .events()
        .iter()
        .any(|e| matches!(e, AuditEvent::Purged { .. })));
    assert_eq!(
        in_store(&f.store, &["innocent words"]),
        vec![0],
        "nothing purged"
    );
    s.purge(own, PurgeRequest::new(PurgeScope::Text("pin 4821".into())))
        .unwrap();
}

/// `and_remove` is two store transactions: a crash after the deletion leaves the purge
/// undone. Retrying the same request completes it (the deletion finds nothing), and a
/// purge repeated after a complete one changes nothing.
#[test]
fn and_remove_retried_after_a_crash_completes_and_is_idempotent() {
    let f = fx("retry");
    let s = f.open("a.md", "a\n");
    let own = join(&s, person("own"));
    let ed = join(&s, person("ed"));
    let sec = secret();
    s.replace_text(ed, ROOT, &format!("a {sec}\nb\n")).unwrap();
    // the crash window: the deletion landed (an ordinary change), the purge didn't
    s.replace_text(own, ROOT, "a \nb\n").unwrap();
    assert_eq!(
        in_store(&f.store, &[&sec]),
        vec![0],
        "history still holds it"
    );
    let req = PurgeRequest::new(PurgeScope::Text(sec.clone())).and_remove();
    let out = s.purge(own, req.clone()).unwrap();
    assert_eq!(out.removed, None, "nothing left to delete");
    assert_eq!(out.live_matches, 0);
    assert!(out.counts.revert_hunks > 0);
    assert_eq!(in_store(&f.store, &[&sec]), Vec::<usize>::new());
    let version = s.status().unwrap().version;
    // again: no change, no content redacted, same text and version
    let again = s.purge(own, req).unwrap();
    assert_eq!(again.removed, None);
    assert_eq!(
        (
            again.counts.revert_hunks,
            again.counts.versions,
            again.counts.bytes
        ),
        (0, 0, 0)
    );
    assert_eq!(s.status().unwrap().version, version);
    assert_eq!(text(&s, ed), "a \nb\n");
    assert_eq!(in_store(&f.store, &[&sec]), Vec::<usize>::new());
}

/// Undo spec U19 / m-g: after a purge, `revert` of the purged change is a conflict of
/// kind `erased`; the control (no purge) reverts cleanly. A purge cleared the stacks, so
/// half a secret typed in two batches can't come back through undo either.
#[test]
fn revert_over_purged_text_reports_erased() {
    let run = |purged: bool| {
        let f = fx(if purged { "erased" } else { "erased-ctl" });
        let s = f.open("a.md", "a\n");
        let own = join(&s, person("own"));
        let ed = join(&s, person("ed"));
        let sec = secret();
        let (h1, h2) = sec.split_at(32);
        let c1 = s
            .replace_text(ed, ROOT, &format!("a\n{h1}"))
            .unwrap()
            .version;
        s.replace_text(ed, ROOT, &format!("a\n{h1}{h2}\n")).unwrap();
        if purged {
            purge(&s, own, PurgeScope::Text(sec.clone()));
            assert_eq!(s.undo(ed, 1).unwrap(), UndoOutcome::ClearedByPurge);
            assert_eq!(text(&s, ed), format!("a\n{sec}\n"), "live text stays");
        }
        s.revert(ed, c1, false).unwrap()
    };
    match run(true) {
        UndoOutcome::Conflicts { conflicts, .. } => {
            assert!(conflicts.iter().all(|c| c.kind == ConflictKind::Erased));
            assert!(conflicts
                .iter()
                .all(|c| c.base.is_empty() && c.ours.is_empty()));
        }
        o => panic!("expected an erased conflict, got {o:?}"),
    }
    assert!(
        !matches!(run(false), UndoOutcome::Conflicts { .. }),
        "without the purge the same revert is clean"
    );
}

/// A version a purge cut a span from is no `write --base`: the base is gone (exit 8, not a
/// wrong merge); the restore of it restores everything else.
#[test]
fn a_redacted_version_is_no_merge_base_and_restores_without_the_span() {
    let f = fx("base");
    let s = f.open("a.md", "a\n");
    let own = join(&s, person("own"));
    let ed = join(&s, person("ed"));
    let sec = secret();
    s.replace_text(ed, ROOT, &format!("a\n{sec}\nb\n")).unwrap();
    let vid = s.name_version(ed, "v", None).unwrap();
    let at = s
        .versions(own)
        .unwrap()
        .into_iter()
        .find(|v| v.id == vid)
        .unwrap()
        .seq;
    s.replace_text(ed, ROOT, "a\nb\nc\n").unwrap();
    purge(&s, own, PurgeScope::Text(sec.clone()));
    // the read-base cache was dropped, and the stored version is redacted
    assert_eq!(
        s.write_base(ed, ROOT, at, "a\nX\nb\n").unwrap(),
        WriteOutcome::BaseExpired
    );
    s.restore(own, vid).unwrap();
    assert_eq!(text(&s, ed), "a\n\nb\n");
}

// ---------- rights and audit ----------

/// K-PURGE-1: owner only, refused with a typed reason and audited; nothing changes. The
/// audit of a purge is metadata only: no event (and no `Debug` of the request) holds the
/// text.
#[test]
fn purge_is_owner_only_and_audited_without_content() {
    let f = fx("owner");
    let s = f.open("a.md", "a\n");
    let own = join(&s, person("own"));
    let ed = join(&s, person("ed"));
    let sec = secret();
    s.replace_text(ed, ROOT, &format!("a {sec}\n")).unwrap();
    s.replace_text(ed, ROOT, "a\n").unwrap();
    let req = PurgeRequest::new(PurgeScope::Text(sec.clone()));
    assert_eq!(
        s.purge(ed, req.clone()).err(),
        Some(SessionError::Forbidden { need: Need::Owner })
    );
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::Refused { action: Action::Purge, need: Need::Owner, pid, .. } if pid == "ed"
    )));
    assert_eq!(in_store(&f.store, &[&sec]), vec![0], "nothing was purged");
    assert!(!format!("{req:?}").contains(&sec[..8]));
    s.purge(own, req).unwrap();
    let ev = f
        .events()
        .into_iter()
        .find(|e| matches!(e, AuditEvent::Purged { .. }))
        .unwrap();
    match &ev {
        AuditEvent::Purged {
            by, scope, counts, ..
        } => {
            assert_eq!(by, "own");
            assert_eq!(*scope, PurgedScope::Text { bytes: sec.len() });
            assert!(counts.revert_hunks > 0);
        }
        _ => unreachable!(),
    }
    let all = format!("{:?}", f.events());
    assert!(!all.contains(&sec[..8]), "no event holds the secret");
}

// ---------- keyed hashes ----------

/// K-PURGE-13: the stored synced hash is keyed (never the plain SHA-256 of the file), a
/// purge replaces the key (the hash of the same file changes), and an echo of a write made
/// before the purge is not taken for our own: it is an outside edit. The control: before
/// the purge, the same old write is an echo.
#[test]
fn keyed_sync_hashes_rotate_on_purge() {
    let f = fx("keyed");
    let s = f.open("a.md", "v1\n");
    let own = join(&s, person("own"));
    let ed = join(&s, person("ed"));
    let id = f.id("a.md");
    let p = f.path("a.md");
    s.replace_text(ed, ROOT, "v2\n").unwrap();
    s.flush(ed).unwrap();
    // a keyed hash: a tag byte and an HMAC (the plain SHA-256 would be 32 bytes; the
    // unit tests check the HMAC is not the plain digest)
    let h0 = f.store.meta(id).unwrap().synced_hash.unwrap();
    assert_eq!((h0.len(), h0[0]), (33, 1));
    let old = b"v2\n".to_vec();
    s.replace_text(ed, ROOT, "v3\n").unwrap();
    s.flush(ed).unwrap();
    // control: an app saving our older write back is an echo
    fs::write(&p, &old).unwrap();
    assert_eq!(s.file_changed().unwrap(), DiskCheck::OwnWrite);
    s.flush(ed).unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"v3\n");
    let h1 = f.store.meta(id).unwrap().synced_hash.unwrap();
    purge(&s, own, PurgeScope::History);
    let h2 = f.store.meta(id).unwrap().synced_hash.unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"v3\n", "same file");
    assert_ne!(h1, h2, "the purge replaced the key");
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Unchanged);
    // the pre-purge write comes back: an outside edit now
    fs::write(&p, &old).unwrap();
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(text(&s, ed), "v2\n");
}

/// Own-write hashes expire (K-PURGE-13: 60 s): after that, an older write of ours coming
/// back is an outside edit.
#[test]
fn own_write_hashes_expire() {
    let f = fx("expire");
    let s = f.open("a.md", "v1\n");
    let ed = join(&s, person("ed"));
    let p = f.path("a.md");
    s.replace_text(ed, ROOT, "v2\n").unwrap();
    s.flush(ed).unwrap();
    s.replace_text(ed, ROOT, "v3\n").unwrap();
    s.flush(ed).unwrap();
    f.advance(59_000);
    fs::write(&p, b"v2\n").unwrap();
    assert_eq!(s.file_changed().unwrap(), DiskCheck::OwnWrite, "control");
    s.flush(ed).unwrap();
    f.advance(2_000);
    fs::write(&p, b"v2\n").unwrap();
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
}

// ---------- participant erasure ----------

/// K-PURGE-7 / U20: the pid is replaced by a pseudonym everywhere the store attributes
/// (log, changes, versions, approvals, the client table), counts kept; their content goes;
/// their connections are removed. Refusals: an empty or reserved pseudonym, self-erasure.
#[test]
fn participant_erasure() {
    let f = fx("erase");
    let pid = "bob-the-agent-0042";
    let s = f.open("a.md", "a\n");
    let own = join(&s, person("own"));
    let bob = join(&s, agent(pid));
    let sec = secret();
    s.replace_text(bob, ROOT, &format!("a\n{sec}\n")).unwrap();
    s.name_version(own, "after bob", None).unwrap();
    s.replace_text(own, ROOT, "a\nb\n").unwrap();
    let id = f.id("a.md");
    assert_eq!(in_store(&f.store, &[pid, &sec]), vec![0, 1]);
    let changes = s.log(own, &Default::default()).unwrap().len();
    for (p, ps) in [(pid, ""), (pid, "kit:x"), ("own", "anon")] {
        assert!(matches!(
            s.purge(
                own,
                PurgeRequest::new(PurgeScope::Participant {
                    pid: p.into(),
                    pseudonym: ps.into()
                })
            ),
            Err(SessionError::BadParticipant(_))
        ));
    }
    let out = purge(
        &s,
        own,
        PurgeScope::Participant {
            pid: pid.into(),
            pseudonym: "erased-1".into(),
        },
    );
    assert!(out.counts.attributions > 0);
    assert_eq!(in_store(&f.store, &[pid, &sec]), Vec::<usize>::new());
    assert!(s.touch(bob).is_err(), "their connection was removed");
    assert_eq!(s.log(own, &Default::default()).unwrap().len(), changes);
    assert!(s
        .log(own, &Default::default())
        .unwrap()
        .iter()
        .any(|i| matches!(
            i,
            k2_mpc_base::session::LogItem::Change { info, .. } if info.author == "erased-1"
        )));
    assert!(f.store.client_owned_by(id, "erased-1").unwrap().is_some());
    assert!(f.store.client_owned_by(id, pid).unwrap().is_none());
}

// ---------- tier 0 (K-T0-9) ----------

fn open_blob(f: &Fx, name: &str) -> Arc<BlobSession> {
    f.mgr
        .open_blob_file(Arc::new(AnyFile), &key(name), &f.path(name), &f.ws)
        .unwrap()
}

fn app_save(path: &Path, bytes: &[u8]) {
    let tmp = path.with_file_name(".app-save.tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

/// Every file under `dir`, recursively.
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

fn blob_dir_holds(f: &Fx, needle: &str) -> bool {
    let blobs = f._dir.0.join("blobs");
    files_under(&blobs)
        .iter()
        .any(|p| !in_file(p, &[needle]).is_empty())
}

/// Per-blob purge: the current version is refused; an older one keeps its entry without
/// its blob (bytes removed now, no GC grace); its conflict copy (whose bytes are that
/// blob, compared by keyed id) is deleted; reading it fails; owner only. The control:
/// the bytes are in the blob folder and the copy before the purge.
#[test]
fn tier0_purge_one_blob_version_and_its_conflict_copy() {
    let f = fx("t0");
    let name = "report.pdf";
    let path = f.path(name);
    let sec = secret();
    let v1_bytes = format!("%PDF-1.7 v1 {sec}\n");
    fs::write(&path, &v1_bytes).unwrap();
    let s = open_blob(&f, name);
    let own = s.join(person("own"), Stay::Held).unwrap().conn;
    let ed = s.join(person("ed"), Stay::Held).unwrap().conn;
    let v1 = s.status().unwrap().version;
    // a desktop save under ed's lease: v1 becomes a conflict copy
    s.acquire_lease(ed, None).unwrap();
    f.advance(1_000);
    app_save(&path, b"%PDF-1.7 v2\n");
    let DiskCheck::Conflict {
        kept,
        copy,
        version,
    } = s.file_changed().unwrap()
    else {
        panic!("expected a conflict");
    };
    assert_eq!(kept, v1);
    let copy = copy.unwrap();
    assert_eq!(s.conflict_copies(own).unwrap(), vec![copy.clone()]);
    assert!(blob_dir_holds(&f, &sec), "control: the blob holds it");
    assert_eq!(
        in_file(&copy, &[&sec]),
        vec![0],
        "control: the copy holds it"
    );
    // refusals: the current version; a non-owner
    assert_eq!(
        s.purge(own, Some(version)).err(),
        Some(SessionError::Store(StoreError::CurrentVersion(
            f.store.find(&key(name)).unwrap().unwrap(),
            version
        )))
    );
    assert_eq!(
        s.purge(ed, Some(v1)).err(),
        Some(SessionError::Forbidden { need: Need::Owner })
    );
    s.release_lease(ed).unwrap();
    let out = s.purge(own, Some(v1)).unwrap();
    assert_eq!(out.counts.blob_versions, 1);
    assert_eq!(out.counts.blobs_removed, 1);
    assert_eq!(out.counts.conflict_copies, 1);
    assert!(!copy.exists(), "the conflict copy is gone");
    assert!(!blob_dir_holds(&f, &sec), "the blob's bytes are gone");
    assert_eq!(in_store(&f.store, &[&sec]), Vec::<usize>::new());
    let vs = s.versions(own).unwrap();
    let (_, purged) = vs.iter().find(|(id, _)| *id == v1).unwrap();
    assert!(purged.redacted);
    assert_eq!(purged.size, v1_bytes.len() as u64, "lengths survive");
    assert!(matches!(
        s.read_version(own, v1).err(),
        Some(SessionError::Store(StoreError::PurgedVersion(..)))
    ));
    assert!(matches!(
        s.restore(own, v1).err(),
        Some(SessionError::Store(StoreError::PurgedVersion(..)))
    ));
    // the current content and the file are untouched
    assert_eq!(fs::read(&path).unwrap(), b"%PDF-1.7 v2\n");
}

/// A doc's whole blob history: every version but the current, with an audit event; a blob
/// another doc shares stays and is reported.
#[test]
fn tier0_purge_whole_history_reports_shared_blobs() {
    let f = fx("t0-all");
    let sec = secret();
    let old = format!("old {sec}\n");
    fs::write(f.path("a.bin"), &old).unwrap();
    fs::write(f.path("b.bin"), &old).unwrap(); // same bytes, another doc
    let a = open_blob(&f, "a.bin");
    let _b = open_blob(&f, "b.bin");
    let own = a.join(person("own"), Stay::Held).unwrap().conn;
    let v = a.status().unwrap().version;
    a.write_bytes(own, b"new\n", v).unwrap();
    let out = a.purge(own, None).unwrap();
    assert_eq!(out.counts.blob_versions, 1);
    assert_eq!(out.counts.blobs_removed, 0);
    assert!(out.not_reached.contains(&NotReached::SharedBlobs(1)));
    assert!(blob_dir_holds(&f, &sec), "b.bin still uses those bytes");
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::Purged {
            scope: PurgedScope::BlobVersions { version: None },
            ..
        }
    )));
    let mut r = a.read(own).unwrap();
    let mut cur = Vec::new();
    r.reader.read_to_end(&mut cur).unwrap();
    assert_eq!(cur, b"new\n");
}

// ---------- at scale ----------

fn scale() -> usize {
    std::env::var("MPC_ERASURE_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n: &usize| *n > 0)
        .unwrap_or(1)
}

/// The gate at size: many docs and changes around a doc whose history holds secrets; a
/// history purge of that doc; a scan of the whole store file finds none of them, and the
/// other docs' content is intact. `MPC_ERASURE_SCALE` sizes it (module docs).
#[test]
fn purge_at_scale_leaves_no_bytes_behind() {
    let n = scale();
    let f = fx("scale");
    let s = f.open("target.md", "target\n");
    let own = join(&s, person("own"));
    let ed = join(&s, person("ed"));
    let secrets: Vec<String> = (0..4 * n.min(64)).map(|_| secret()).collect();
    let mut live = String::from("target\n");
    for (i, sec) in secrets.iter().enumerate() {
        s.replace_text(ed, ROOT, &format!("{live}{sec}\n")).unwrap();
        if i % 7 == 0 {
            s.name_version(ed, &format!("n{i}"), None).unwrap();
        }
        s.replace_text(ed, ROOT, &live).unwrap();
        live.push_str(&format!("line {i}\n"));
        s.replace_text(ed, ROOT, &live).unwrap();
    }
    let mut fillers = Vec::new();
    for d in 0..10 * n {
        let name = format!("f{d}.md");
        let o = f.open(&name, &format!("filler {d}\n"));
        let c = join(&o, person("ed"));
        let mut t = format!("filler {d}\n");
        for k in 0..20 {
            t.push_str(&format!("doc {d} line {k} {}\n", "x".repeat(40)));
            o.replace_text(c, ROOT, &t).unwrap();
        }
        fillers.push((name, t));
    }
    let needles: Vec<&str> = secrets.iter().map(String::as_str).collect();
    assert_eq!(in_store(&f.store, &needles).len(), needles.len(), "control");
    let out = purge(&s, own, PurgeScope::History);
    let r = out.rewrite.unwrap();
    assert!(r.bytes_after > 0);
    assert_eq!(in_store(&f.store, &needles), Vec::<usize>::new());
    assert_eq!(text(&s, ed), live);
    for (name, t) in &fillers {
        let o = f.mgr.get(&key(name)).unwrap();
        let c = join(&o, person("ed"));
        assert_eq!(&text(&o, c), t);
    }
}
