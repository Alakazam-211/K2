//! A grid change that re-identifies elements re-syncs the disk replica after its store commit. If
//! that fails, the change has still landed: the caller gets `Ok`, so there is nothing to retry, and
//! it can be undone. The failure goes to `AuditEvent::DiskError`. The replica stays behind the doc
//! until an outside save or a write-back re-syncs it first, so a later merge has the right base.
//! Public API; the failure is a pack whose `encode` fails, which a pack can do.
//!
//! The same failure elsewhere. A purge goes on without its re-sync and says what it left
//! (`NotReached::File`). An outside save that fails before the store reloads the doc, and comes in
//! at the next check. One that fails after the store returns `Ok`, and doesn't land twice.
//!
//! `MPC_STRUCT_MUTANT` plants a fault for the whole suite (`k2_mpc_base::differential`, the
//! `differential` feature, on for these tests), each caught by the tests of its section:
//! `resync-returns-the-error` is `ResyncMutant::ReturnsTheError`, `purge-stops-at-the-resync` is
//! `PurgeStopsAtTheResync`, `ingest-keeps-the-doc` is `IngestKeepsTheDoc`, and
//! `settle-returns-the-error` is `SettleReturnsTheError`. Unset, the tests run as is. Any other value
//! stops the suite. `MPC_LINE_MUTANT` plants `LineMutant`'s faults for the tests of "when it can't
//! tell, no line number".

#[path = "grid_pack/mod.rs"]
mod grid_pack;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::{
    with_apply_fault, with_line_mutant, with_resync_mutant, ApplyFault, LineMutant, ResyncMutant,
};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    AuditEvent, ClaimEnd, ClaimInfo, ClaimSpan, Config, ConnId, Declined, DiskCheck, DiskState,
    EditOptions, EditWithError, GridWrite, GridWriteOptions, Host, LineRange, NotReached, NoteKind,
    Pack, PackContent, PackError, Participant, ParticipantKind, PurgeRequest, PurgeScope, Rights,
    Session, SessionError, SessionManager, Stay, TextFilePack, UndoOutcome, View,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::{Table, TableError, ROWS};
use k2_mpc_base::tools::PackVerb;

fn under_struct_mutant<R>(f: impl FnOnce() -> R) -> R {
    let m = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("resync-returns-the-error") => with_resync_mutant(ResyncMutant::ReturnsTheError, f),
        Ok("purge-stops-at-the-resync") => {
            with_resync_mutant(ResyncMutant::PurgeStopsAtTheResync, f)
        }
        Ok("ingest-keeps-the-doc") => with_resync_mutant(ResyncMutant::IngestKeepsTheDoc, f),
        Ok("settle-returns-the-error") => {
            with_resync_mutant(ResyncMutant::SettleReturnsTheError, f)
        }
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this suite plants"),
        Err(_) => f(),
    }
}

/// `MPC_LINE_MUTANT` (`k2_mpc_base::differential::LineMutant`, the setting echoed) for this file's
/// line number tests. Faults this file can't see run as is.
fn under_line_mutant<R>(f: impl FnOnce() -> R) -> R {
    let m = std::env::var("MPC_LINE_MUTANT");
    println!("MPC_LINE_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("ln-keeps-line-map") => with_line_mutant(LineMutant::LnKeepsLineMap, f),
        Ok("ln-goes-on-unrendered") => with_line_mutant(LineMutant::LnGoesOnUnrendered, f),
        Ok("claim-lines-go-on-unrendered") => {
            with_line_mutant(LineMutant::ClaimLinesGoOnUnrendered, f)
        }
        Ok("conflict-lines-fall-back") => with_line_mutant(LineMutant::ConflictLinesFallBack, f),
        Ok("row-list-keeps-lines") => with_line_mutant(LineMutant::RowListKeepsLines, f),
        Ok("conflict-keeps-lines") => with_line_mutant(LineMutant::ConflictKeepsLines, f),
        _ => f(),
    }
}

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
}
impl Host for TestHost {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

/// A pack whose `encode` can be made to fail, after a number of calls that pass.
struct Failing<P>(P, Arc<AtomicBool>, Arc<AtomicU64>);
impl<P: Pack> Pack for Failing<P> {
    fn schema(&self) -> Schema {
        self.0.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        self.0.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let left = self.2.load(Ordering::SeqCst);
        if left > 0 {
            self.2.store(left - 1, Ordering::SeqCst);
        } else if self.1.load(Ordering::SeqCst) {
            return Err(PackError("a planted failure".into()));
        }
        self.0.encode(c, codec)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        self.0.verbs()
    }
}

struct Fx {
    dir: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    mgr: SessionManager,
    fail: Arc<AtomicBool>,
    pass: Arc<AtomicU64>,
}
impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const TEXT: TextFilePack = TextFilePack::new("resync", 1 << 20);
const GRID: &str = "id,city\nk1,Oslo\nk2,Rome\n";

fn config() -> Config {
    let mut c = Config::default();
    c.edit_burst = 100_000;
    c.edit_rate_per_s = 100_000;
    c
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "t.csv".into(),
    }
}

impl Fx {
    fn new(tag: &str, body: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-resync-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("files")).unwrap();
        std::fs::write(dir.join("files/t.csv"), body).unwrap();
        let host = Arc::new(TestHost {
            now: AtomicU64::new(1_000_000),
            events: Mutex::new(Vec::new()),
        });
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
        let mgr = SessionManager::new(store.clone(), host.clone(), config());
        Fx {
            dir,
            host,
            store,
            mgr,
            fail: Arc::new(AtomicBool::new(false)),
            pass: Arc::new(AtomicU64::new(0)),
        }
    }
    fn file(&self) -> PathBuf {
        self.dir.join("files/t.csv")
    }
    fn grid(&self) -> Arc<Session> {
        self.mgr
            .open_file(
                Arc::new(Failing(
                    grid_pack::GridLite,
                    self.fail.clone(),
                    self.pass.clone(),
                )),
                &key(),
                &self.file(),
                &self.dir.join("files"),
            )
            .unwrap()
    }
    fn text(&self) -> Arc<Session> {
        self.mgr
            .open_file(
                Arc::new(Failing(TEXT, self.fail.clone(), self.pass.clone())),
                &key(),
                &self.file(),
                &self.dir.join("files"),
            )
            .unwrap()
    }
    fn failing(&self, on: bool) {
        self.fail.store(on, Ordering::SeqCst);
    }
    /// `encode` passes `n` more times, then fails.
    fn failing_after(&self, n: u64) {
        self.pass.store(n, Ordering::SeqCst);
        self.failing(true);
    }
    /// A restart: a new manager on the same store, nothing written back.
    fn restart(&mut self) {
        self.mgr = SessionManager::new(self.store.clone(), self.host.clone(), config());
    }
    /// Another program saves the file.
    fn outside_save(&self, body: &str) {
        self.host.now.fetch_add(500, Ordering::SeqCst);
        std::fs::write(self.file(), body).unwrap();
    }
    fn bytes(&self) -> String {
        std::fs::read_to_string(self.file()).unwrap()
    }
    fn disk_errors(&self) -> Vec<String> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AuditEvent::DiskError { error, .. } => Some(error.clone()),
                _ => None,
            })
            .collect()
    }
    fn purged(&self) -> usize {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, AuditEvent::Purged { .. }))
            .count()
    }
}

fn person(s: &Session, pid: &str) -> ConnId {
    s.join(
        Participant::new(
            pid,
            pid.to_uppercase(),
            "#123456",
            ParticipantKind::Person,
            "t",
        ),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn op(
    s: &Session,
    c: ConnId,
    g: impl FnOnce(&mut Table) -> Result<Vec<Edit>, TableError>,
) -> Result<(), SessionError> {
    match s.edit_ops_with(c, &EditOptions::new(), |v: &View| {
        let mut t = v.table().map_err(|e| e.to_string())?;
        g(&mut t).map_err(|e| e.to_string())
    }) {
        Ok(_) => Ok(()),
        Err(EditWithError::Session(e)) => Err(e),
        Err(EditWithError::Compute(e)) => panic!("table op: {e}"),
    }
}

fn live_rows(s: &Session, c: ConnId) -> Vec<Key> {
    let mut rows = Vec::new();
    s.edit_ops_with(c, &EditOptions::new(), |v: &View| {
        rows = v.table().map_err(|e| e.to_string())?.live_rows();
        Ok::<_, String>(Vec::new())
    })
    .unwrap();
    rows
}

fn insert_oslo(s: &Session, c: ConnId) -> Result<(), SessionError> {
    op(s, c, |t| {
        t.insert_row(Anchor::Start, &["k1".into(), "Oslo".into()])
            .map(|x| x.1)
    })
}

fn text_of(s: &Session, c: ConnId) -> String {
    String::from_utf8(s.read(c).unwrap().bytes).unwrap()
}

// ---------- a change that is durable ----------

#[test]
fn a_durable_change_with_a_failed_resync_returns_ok_and_is_reported() {
    under_struct_mutant(|| {
        let f = Fx::new("ok", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        let v = s.status().unwrap().version;
        f.failing(true);
        let r = op(&s, p, |t| t.delete_row(&k("r1")));
        assert!(r.is_ok(), "told a durable change failed: {r:?}");
        assert!(s.status().unwrap().version > v, "it landed");
        let e = f.disk_errors();
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(
            e[0].contains("re-syncing the disk replica") && e[0].contains("a planted failure"),
            "{e:?}"
        );
        // and it goes on the undo stack
        let u = s.undo(p, 1).unwrap();
        assert!(matches!(u, UndoOutcome::Applied { .. }), "{u:?}");
        f.failing(false);
        assert_eq!(text_of(&s, p), GRID, "undone");
    });
}

/// The harm the error did: a host that retries on an error. With `Ok` there is nothing to retry, so
/// the session sees the change once; this loop is what a caller told it failed would do.
#[test]
fn a_host_that_retries_on_an_error_makes_a_durable_change_once() {
    under_struct_mutant(|| {
        let f = Fx::new("retry", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        let before = live_rows(&s, p).len();
        f.failing(true);
        let first = insert_oslo(&s, p);
        f.failing(false);
        if first.is_err() {
            insert_oslo(&s, p).unwrap();
        }
        assert_eq!(live_rows(&s, p).len(), before + 1, "made twice: {first:?}");
    });
}

/// The owner's claim on the rows it touched ends at its own change, not at the next one.
#[test]
fn a_durable_change_with_a_failed_resync_ends_its_claim() {
    under_struct_mutant(|| {
        let f = Fx::new("claim", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        let span = ClaimSpan::Rows {
            first: k("r1"),
            last: k("r2"),
        };
        let id = s.claim_span(p, span, None, None, false).unwrap().id;
        f.failing(true);
        let r = op(&s, p, |t| t.delete_row(&k("r1")));
        assert!(r.is_ok(), "told a durable change failed: {r:?}");
        let ended: Vec<_> = f
            .host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AuditEvent::ClaimEnded { claim, how, .. } if *claim == id => Some(*how),
                _ => None,
            })
            .collect();
        assert_eq!(ended, [ClaimEnd::Edited]);
    });
}

// ---------- the replica after a failed re-sync ----------

/// The doc back at the file's bytes under a new row key, with the re-sync failed: the replica still
/// holds the old key. The row on line 1 is `r3` in the doc, `r1` in the replica.
fn doc_at_the_files_bytes_under_a_new_key(f: &Fx, s: &Session, p: ConnId) {
    op(s, p, |t| t.delete_row(&k("r1"))).unwrap();
    f.failing(true);
    insert_oslo(s, p).unwrap();
    f.failing(false);
    assert_eq!(text_of(s, p), GRID, "the doc renders the file's bytes");
    assert_eq!(live_rows(s, p), [k("r3"), k("r2")], "under a new key");
    assert_eq!(f.disk_errors().len(), 1, "the re-sync failed");
}

/// What a merge on the right base gives: the save lands on the live row, once.
fn assert_merged_on_the_live_row(f: &Fx, s: &Session, p: ConnId) {
    assert_eq!(
        text_of(s, p),
        SAVED,
        "the doc took the save on its live row"
    );
    assert_eq!(live_rows(s, p), [k("r3"), k("r2")], "no row added or lost");
    s.flush(p).unwrap();
    assert_eq!(f.bytes(), SAVED, "the file holds it");
    assert_eq!(s.audit_disk_sync().unwrap(), [], "the replica is the doc");
    assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
}

const SAVED: &str = "id,city\nk1,OSLO\nk2,Rome\n";

#[test]
fn an_outside_save_after_a_failed_resync_merges_on_the_live_row() {
    under_struct_mutant(|| {
        let f = Fx::new("save", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        doc_at_the_files_bytes_under_a_new_key(&f, &s, p);
        f.outside_save(SAVED);
        let c = s.file_changed().unwrap();
        assert!(matches!(c, DiskCheck::Ingested { .. }), "{c:?}");
        assert_merged_on_the_live_row(&f, &s, p);
    });
}

/// `encode` still failing when the save arrives: it isn't taken on a stale base; it stays in the
/// file, and comes in when the check can re-sync.
#[test]
fn an_outside_save_while_the_resync_still_fails_waits_and_then_merges() {
    under_struct_mutant(|| {
        let f = Fx::new("save-wait", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        doc_at_the_files_bytes_under_a_new_key(&f, &s, p);
        f.failing(true);
        f.outside_save(SAVED);
        let e = s.file_changed().expect_err("the check can't re-sync");
        assert!(matches!(e, SessionError::Pack(_)), "{e:?}");
        f.failing(false);
        assert_eq!(text_of(&s, p), GRID, "nothing merged");
        assert_eq!(f.bytes(), SAVED, "the save is still in the file");
        let c = s.file_changed().unwrap();
        assert!(matches!(c, DiskCheck::Ingested { .. }), "{c:?}");
        assert_merged_on_the_live_row(&f, &s, p);
    });
}

/// The same save found by the write-back, which looks at the file first, and doesn't overwrite it.
#[test]
fn a_write_back_after_a_failed_resync_takes_the_save_first() {
    under_struct_mutant(|| {
        let f = Fx::new("save-wb", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        doc_at_the_files_bytes_under_a_new_key(&f, &s, p);
        f.outside_save(SAVED);
        s.flush(p).unwrap();
        assert_merged_on_the_live_row(&f, &s, p);
    });
}

/// With no save, the write-back re-syncs: the replica is the doc again.
#[test]
fn a_write_back_after_a_failed_resync_brings_the_replica_up() {
    under_struct_mutant(|| {
        let f = Fx::new("wb", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        doc_at_the_files_bytes_under_a_new_key(&f, &s, p);
        s.flush(p).unwrap();
        assert_eq!(f.bytes(), GRID);
        assert_eq!(s.audit_disk_sync().unwrap(), [], "the replica is the doc");
        assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());
    });
}

/// A restart after the failed re-sync, nothing written back: the store's replica is the old one, and the next open re-syncs
/// before it takes the save.
#[test]
fn an_outside_save_after_a_failed_resync_and_a_restart_merges_on_the_live_row() {
    under_struct_mutant(|| {
        let mut f = Fx::new("restart", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        doc_at_the_files_bytes_under_a_new_key(&f, &s, p);
        drop(s);
        f.restart();
        f.outside_save(SAVED);
        let s = f.grid();
        let p = person(&s, "p");
        assert_merged_on_the_live_row(&f, &s, p);
    });
}

// ---------- a purge ----------

/// A purge's write-back fails with `encode`, so the file isn't reached, and then so does its
/// re-sync. It goes on: the history is erased, and the report says what's left. What's left in the
/// store is the replica, not freed pages: a rewrite doesn't take it, the next write-back does.
#[test]
fn a_purge_that_cannot_render_the_doc_erases_the_history_and_says_what_it_left() {
    under_struct_mutant(|| {
        let sec = "0123456789abcdef-the-secret";
        let f = Fx::new("purge", &format!("keep\n{sec}\n"));
        let s = f.text();
        let own = person(&s, "own");
        let ed = person(&s, "ed");
        // in the history as well as the live text
        s.edit_with(ed, TextFilePack::ROOT, |t| {
            Ok::<_, ()>(format!("{t}again {sec}\n"))
        })
        .unwrap();
        let hits = |f: &Fx| f.store.scan_for(&[sec.as_bytes()]).unwrap().len();
        assert!(hits(&f) > 1, "the store holds it, more than once");
        let req = || PurgeRequest::new(PurgeScope::Text(sec.to_owned())).and_remove();

        f.failing(true);
        let r = s.purge(own, req());
        assert!(r.is_ok(), "the purge stopped: {r:?}");
        let out = r.unwrap();
        assert!(out.removed.is_some(), "the live removal landed");
        assert_eq!(out.live_matches, 0);
        assert!(out.not_reached.contains(&NotReached::File), "{out:?}");
        assert_eq!(f.purged(), 1, "the purge is on the record");
        assert!(
            f.disk_errors().iter().any(|e| e.contains("for a purge")),
            "{:?}",
            f.disk_errors()
        );
        assert!(f.bytes().contains(sec), "the file: as the report says");
        assert_eq!(hits(&f), 1, "the history is gone; one copy left");
        f.store.rewrite_file().unwrap();
        assert_eq!(
            hits(&f),
            1,
            "not freed pages: the replica, as the report says"
        );

        // purging again with `encode` still failing: nothing to remove, the same report
        let again = s.purge(own, req()).unwrap();
        assert!(again.removed.is_none(), "nothing applied twice");
        assert!(again.not_reached.contains(&NotReached::File), "{again:?}");

        // the next write-back takes it from the file and the replica
        f.failing(false);
        s.flush(own).unwrap();
        assert!(!f.bytes().contains(sec), "the file");
        f.store.rewrite_file().unwrap();
        assert_eq!(hits(&f), 0, "the replica's copy went with the write-back");
        assert_eq!(s.audit_disk_sync().unwrap(), [], "the replica is the doc");
    });
}

/// The corner the call's comment names: the file missing as well, so the new key leaves the sync
/// point unknown and no re-sync can run before the next save. `reidentify_onto` gives the replica
/// the live keys, so the save lands on the live row. (The doc's bytes are what a person sees: with
/// `reidentify_onto` off too, the duplicate row shows.)
#[test]
fn an_outside_save_after_a_purge_with_the_file_missing_merges_on_the_live_row() {
    under_struct_mutant(|| {
        let f = Fx::new("purge-corner", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        doc_at_the_files_bytes_under_a_new_key(&f, &s, p);
        std::fs::remove_file(f.file()).unwrap();
        assert!(matches!(s.file_changed().unwrap(), DiskCheck::Missing));
        f.failing(true);
        let r = s.purge(p, PurgeRequest::new(PurgeScope::History));
        f.failing(false);
        assert!(r.is_ok(), "the purge stopped: {r:?}");
        f.outside_save(SAVED);
        let c = s.file_changed().unwrap();
        assert!(matches!(c, DiskCheck::Ingested { .. }), "{c:?}");
        assert_merged_on_the_live_row(&f, &s, p);
    });
}

// ---------- an outside save that fails before the store ----------

/// `holds_file` renders the doc after it took the save: failing there, the doc reloads, the save
/// stays in the file, and the next check logs it. A restart keeps it.
#[test]
fn an_outside_save_that_fails_before_the_store_reloads_and_the_next_check_logs_it() {
    under_struct_mutant(|| {
        let mut f = Fx::new("ingest", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        let v = s.status().unwrap().version;
        f.failing(true);
        f.outside_save(SAVED);
        let e = s.file_changed().expect_err("the check fails");
        assert!(matches!(e, SessionError::Pack(_)), "{e:?}");
        f.failing(false);
        assert_eq!(s.status().unwrap().version, v, "nothing logged");
        assert_eq!(
            text_of(&s, p),
            GRID,
            "the doc kept a save the store never got"
        );
        assert_eq!(f.bytes(), SAVED, "the save is still in the file");

        let c = s.file_changed().unwrap();
        assert!(
            matches!(c, DiskCheck::Ingested { version } if version == v + 1),
            "the next check logs it: {c:?}"
        );
        assert_eq!(text_of(&s, p), SAVED);
        s.flush(p).unwrap();
        assert!(s.audit().unwrap().is_clean(), "{:?}", s.audit());

        drop(s);
        f.restart();
        let s = f.grid();
        let p = person(&s, "p");
        assert_eq!(text_of(&s, p), SAVED, "a restart keeps it");
    });
}

/// The bookkeeping after the doc took the save (`structured_applied`) failing does the same.
#[test]
fn an_outside_saves_bookkeeping_that_fails_reloads_and_the_next_check_logs_it() {
    under_struct_mutant(|| {
        let f = Fx::new("ingest-book", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        let v = s.status().unwrap().version;
        f.outside_save(SAVED);
        let e = with_apply_fault(ApplyFault::BookkeepingFailsAfter(0), || s.file_changed())
            .expect_err("the check fails");
        assert!(matches!(e, SessionError::Store(_)), "{e:?}");
        assert_eq!(s.status().unwrap().version, v, "nothing logged");
        assert_eq!(
            text_of(&s, p),
            GRID,
            "the doc kept a save the store never got"
        );
        let c = s.file_changed().unwrap();
        assert!(
            matches!(c, DiskCheck::Ingested { version } if version == v + 1),
            "the next check logs it: {c:?}"
        );
        assert_eq!(text_of(&s, p), SAVED);
    });
}

// ---------- an outside save that fails after the store ----------

/// `encode` failing after the store commit, on text, which renders only then. The save has landed:
/// `Ok`, reported, and a second check finds nothing to do, so it lands once. A host sees
/// `Ingested` and a `DiskError`.
#[test]
fn a_text_save_in_the_store_returns_ok_lands_once_and_is_reported() {
    under_struct_mutant(|| {
        let f = Fx::new("settle", "one\ntwo\n");
        let s = f.text();
        let p = person(&s, "p");
        let v = s.status().unwrap().version;
        f.failing(true);
        f.outside_save("ONE\ntwo\n");
        let r = s.file_changed();
        assert!(
            matches!(r, Ok(DiskCheck::Ingested { version }) if version == v + 1),
            "told a save in the store failed: {r:?}"
        );
        let e = f.disk_errors();
        assert!(
            e.len() == 1 && e[0].contains("settling after an outside save"),
            "{e:?}"
        );
        assert!(
            matches!(s.status().unwrap().disk, Some(DiskState::Dirty { .. })),
            "the write-back looks again"
        );
        // a repeat of the check: nothing to take
        assert!(matches!(s.file_changed().unwrap(), DiskCheck::Unchanged));
        assert_eq!(s.status().unwrap().version, v + 1, "it landed once");
        f.failing(false);
        assert_eq!(text_of(&s, p), "ONE\ntwo\n");
        s.flush(p).unwrap();
        assert!(
            matches!(s.status().unwrap().disk, Some(DiskState::Clean)),
            "{:?}",
            s.status().unwrap().disk
        );
        assert_eq!(s.audit_disk_sync().unwrap(), [], "the replica is the doc");
    });
}

/// A save holding what the doc already holds moves only the sync point, and settles the same way.
#[test]
fn a_text_save_of_what_the_doc_holds_returns_ok_when_settling_fails() {
    under_struct_mutant(|| {
        let f = Fx::new("settle-same", "one\ntwo\n");
        let s = f.text();
        let p = person(&s, "p");
        s.edit_with(p, TextFilePack::ROOT, |_| {
            Ok::<_, ()>("ONE\ntwo\n".to_owned())
        })
        .unwrap();
        let v = s.status().unwrap().version;
        f.failing(true);
        f.outside_save("ONE\ntwo\n");
        let r = s.file_changed();
        assert!(matches!(r, Ok(DiskCheck::Unchanged)), "{r:?}");
        assert_eq!(s.status().unwrap().version, v, "no change");
        assert_eq!(f.disk_errors().len(), 1, "{:?}", f.disk_errors());
        f.failing(false);
        s.flush(p).unwrap();
        assert!(matches!(s.status().unwrap().disk, Some(DiskState::Clean)));
    });
}

/// The same on a grid: `holds_file`'s render passes, and the one after the store commit fails.
#[test]
fn a_grid_save_in_the_store_returns_ok_when_settling_fails() {
    under_struct_mutant(|| {
        let f = Fx::new("settle-grid", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        let v = s.status().unwrap().version;
        f.failing_after(1);
        f.outside_save(SAVED);
        let r = s.file_changed();
        assert!(
            matches!(r, Ok(DiskCheck::Ingested { version }) if version == v + 1),
            "told a save in the store failed: {r:?}"
        );
        assert!(matches!(s.file_changed().unwrap(), DiskCheck::Unchanged));
        assert_eq!(s.status().unwrap().version, v + 1, "it landed once");
        f.failing(false);
        assert_eq!(text_of(&s, p), SAVED);
    });
}

// ---------- when it can't tell, no line number ----------

/// A keyed write as a person makes it, with no label of its own.
fn set_city(s: &Session, p: ConnId, row: &str, v: &str) {
    op(s, p, |t| t.set_cell(&k(row), &k("c2"), v)).unwrap();
}

fn argv(a: &str) -> Vec<String> {
    a.split(' ').map(str::to_owned).collect()
}

fn claim_line_3(s: &Session, p: ConnId) -> Result<ClaimInfo, SessionError> {
    s.claim(
        p,
        LineRange {
            root: ROWS.into(),
            first: 3,
            last: 3,
        },
        None,
        None,
        false,
    )
}

/// A file that can't be rendered might hold a lone CR, so a line number has no checked meaning:
/// `claim --lines` refuses, by id, and leaves nothing. A claim by key doesn't ask.
#[test]
fn a_claim_by_line_refuses_when_the_file_cannot_be_rendered() {
    under_line_mutant(|| {
        let f = Fx::new("claim-lines", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        f.failing(true);
        let before = s.activity(p, 50).unwrap().len();
        let e = claim_line_3(&s, p).expect_err("a claim by line refuses");
        let r = e.report();
        assert_eq!((r.id(), r.exit()), ("MPC-E177", 2), "{e}");
        assert!(
            e.to_string().contains("key") && e.to_string().contains("#N"),
            "names what works: {e}"
        );
        assert_eq!(s.claims(p).unwrap(), Vec::new(), "no claim");
        assert_eq!(s.activity(p, 50).unwrap().len(), before, "no activity");
        let by_key = s.claim_span(
            p,
            ClaimSpan::Rows {
                first: k("r2"),
                last: k("r2"),
            },
            None,
            None,
            false,
        );
        assert!(by_key.is_ok(), "{by_key:?}");
        f.failing(false);
        let c = claim_line_3(&s, p);
        assert!(c.is_ok(), "the same claim once it can render: {c:?}");
    });
}

/// So does `LN`, and it doesn't go on to the line map. What it leaves: nothing. A key and `#N`
/// don't ask, and the refusal's next step works with the render failing.
#[test]
fn an_ln_edit_refuses_when_the_file_cannot_be_rendered() {
    under_line_mutant(|| {
        let f = Fx::new("ln-edit", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        let v = s.status().unwrap().version;
        let entries = s.status().unwrap().log_entries;
        f.failing(true);
        let e = s
            .run_tool(
                p,
                ROWS,
                &argv("cell set --row L3 --col city --value Lima"),
                b"",
            )
            .map(|o| format!("{:?}", o.exit))
            .expect_err("LN refuses");
        let r = e.report();
        assert_eq!((r.id(), r.exit(), e.exit()), ("MPC-E177", 2, 2), "{e}");
        assert!(
            e.to_string().contains("key") && e.to_string().contains("#N"),
            "names what works: {e}"
        );
        assert_eq!(s.status().unwrap().version, v, "nothing applied");
        assert_eq!(s.status().unwrap().log_entries, entries);
        let places = s.blame_structure(p).unwrap();
        assert_eq!(places.rows[1].key, k("r2"), "the next step works");
        for a in [
            "cell set --row r2 --col city --value Lima",
            "cell set --row #2 --col city --value Lima",
        ] {
            let o = s.run_tool(p, ROWS, &argv(a), b"").unwrap();
            assert_eq!(o.exit, 0, "{a}: {o:?}");
        }
        f.failing(false);
        assert_eq!(text_of(&s, p), "id,city\nk1,Oslo\nk2,Lima\n");
        assert_eq!(
            s.run_tool(p, ROWS, &argv("cell get --row L3 --col city"), b"")
                .unwrap()
                .stdout,
            b"Lima\n",
            "LN again once it can render"
        );
    });
}

/// A read verb renders the file first: with the render failing, `row list` and `cell get --row
/// LN` fail there, and nothing moves. If only the check's render fails, `row list` lists its rows
/// with no line numbers, and `cell get --row LN` refuses.
#[test]
fn row_list_and_a_read_by_ln_when_the_file_cannot_be_rendered() {
    under_line_mutant(|| {
        let f = Fx::new("read-lines", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        let before = s.activity(p, 50).unwrap().len();
        f.failing(true);
        for a in ["row list", "cell get --row L3 --col city"] {
            let e = s
                .run_tool(p, ROWS, &argv(a), b"")
                .map(|o| format!("{:?}", o.exit))
                .expect_err("no file to read");
            println!(
                "{a}, render failing: exit {}: {e} ({})",
                e.exit(),
                e.report().id()
            );
            assert!(e.to_string().contains("a planted failure"), "{a}: {e}");
        }
        assert_eq!(s.activity(p, 50).unwrap().len(), before, "nothing moved");

        // the read's own render passes, and the next fails
        f.failing_after(1);
        let o = s.run_tool(p, ROWS, &argv("row list"), b"").unwrap();
        let listing = String::from_utf8_lossy(&o.stdout).into_owned();
        println!("row list, the check's render failing: {listing:?}");
        assert_eq!(
            listing, "r1\t#1\t-\tk1\tOslo\nr2\t#2\t-\tk2\tRome\n",
            "no line numbers"
        );
        f.failing_after(1);
        let e = s
            .run_tool(p, ROWS, &argv("cell get --row L3 --col city"), b"")
            .map(|o| format!("{:?}", o.exit))
            .expect_err("LN refuses");
        println!(
            "cell get --row L3, the check's render failing: {e} ({})",
            e.report().id()
        );
        assert_eq!(e.report().id(), "MPC-E177", "{e}");
    });
}

/// A conflict report with the render failing gives no line, and the conflict all the same.
#[test]
fn a_conflict_report_gives_no_line_when_the_file_cannot_be_rendered() {
    under_line_mutant(|| {
        let f = Fx::new("conflict-lines", GRID);
        let s = f.grid();
        let p = person(&s, "p");
        let base = s.status().unwrap().version;
        set_city(&s, p, "r2", "Lima");
        f.failing(true);
        let r = s
            .write_base_grid(
                p,
                base,
                b"id,city\nk1,Oslo\nk2,Bari\n",
                &GridWriteOptions::default(),
            )
            .unwrap();
        let GridWrite::Conflicts(cs) = &r.outcome else {
            panic!("{r:?}")
        };
        assert_eq!(cs.len(), 1, "{cs:?}");
        assert_eq!(cs[0].lines, None, "it can't tell, so no number");
        assert!(cs[0].to_string().contains("r2"), "{}", cs[0]);
    });
}

/// Flush, purge, unload and tick, with the render working and failing, on a doc holding a lone CR
/// and one with none: they behave the same. A flush has to render the file, so it fails while the
/// render does (`Pack`, as on any doc), and nothing is lost: after a restart, the doc and the file
/// hold the change, and the store's audit is clean.
#[test]
fn a_lone_cr_doc_goes_through_flush_purge_unload_and_tick() {
    under_line_mutant(|| {
        for failing in [false, true] {
            for body in ["id,city\nk1,a\rb\nk2,Rome\n", "id,city\nk1,a b\nk2,Rome\n"] {
                let what = format!("failing {failing}, {body:?}");
                let want = body.replace("Rome", "Lima");
                let mut f = Fx::new("must-succeed", body);
                let s = f.grid();
                let p = person(&s, "p");
                set_city(&s, p, "r2", "Lima");
                f.failing(failing);

                let flush = s.flush(p);
                if failing {
                    assert!(
                        matches!(flush, Err(SessionError::Pack(_))),
                        "{what}: {flush:?}"
                    );
                    assert_eq!(f.bytes(), body, "{what}: the file as it was");
                } else {
                    assert!(flush.is_ok(), "{what}: {flush:?}");
                    assert_eq!(f.bytes(), want, "{what}: the file");
                }

                let purge = s.purge(p, PurgeRequest::new(PurgeScope::History));
                assert!(purge.is_ok(), "{what}: the purge stopped: {purge:?}");
                let out = purge.unwrap();
                assert_eq!(
                    out.not_reached.contains(&NotReached::File),
                    failing,
                    "{what}: {out:?}"
                );

                f.host.now.fetch_add(60_000, Ordering::SeqCst);
                let tick = f.mgr.tick();
                assert!(tick.is_ok(), "{what}: the tick stopped: {tick:?}");

                s.leave(p).unwrap();
                drop(s);
                f.host.now.fetch_add(24 * 3_600_000, Ordering::SeqCst);
                let unloaded = f.mgr.unload_idle();
                assert!(unloaded.is_ok(), "{what}: the unload stopped: {unloaded:?}");
                println!("{what}: purge ok, tick ok, unloaded {unloaded:?}");

                f.failing(false);
                f.restart();
                let s = f.grid();
                let p = person(&s, "p");
                assert_eq!(text_of(&s, p), want, "{what}: the doc");
                s.flush(p).unwrap();
                assert_eq!(f.bytes(), want, "{what}: the file");
                assert!(s.audit().unwrap().is_clean(), "{what}: {:?}", s.audit());
            }
        }
    });
}

/// An outside save that overrides a live value, with `encode` failing at each step in turn: the
/// save is taken once, now or at the next check, and tells the value's author once. Whatever the
/// report's line asks of the pack adds no way to lose it.
#[test]
fn an_overriding_save_lands_once_wherever_rendering_fails() {
    under_line_mutant(|| {
        for n in 0..16 {
            let f = Fx::new("override-sweep", GRID);
            let s = f.grid();
            let p = person(&s, "p");
            op(&s, p, |t| t.set_cell(&k("r2"), &k("c2"), "LIVE")).unwrap();
            f.outside_save("id,city\nk1,Oslo\nk2,FILE\n");
            f.failing_after(n);
            let _ = s.file_changed();
            f.failing(false);
            let _ = s.file_changed();
            assert_eq!(
                text_of(&s, p),
                "id,city\nk1,Oslo\nk2,FILE\n",
                "encode failing after {n} calls"
            );
            let notes = s.take_notes(p).unwrap();
            let told = notes
                .notes
                .iter()
                .filter(|x| x.kind == NoteKind::Overwritten)
                .count();
            assert_eq!(told, 1, "encode failing after {n} calls: {notes:?}");
        }
    });
}
