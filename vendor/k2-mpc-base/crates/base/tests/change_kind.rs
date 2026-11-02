//! `ChangeEvent::kind` (kit v0.10.1): an undo's and a redo's change events say so, live (a
//! subscription) and from `events_since`, before and after a restart (the change records'
//! kind in the store). Public API only.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, ChangeEvent, ChangeKind, Config, ConnId, Host, Participant, ParticipantKind,
    Rights, Session, SessionManager, Stay, TextFilePack, UndoOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::Edit;

const ROOT: &str = TextFilePack::ROOT;

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct TmpDir(std::path::PathBuf);

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(dir: &std::path::Path, host: Arc<TestHost>) -> (SessionManager, Arc<Session>) {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host, Config::default());
    let s = mgr
        .open(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &DocKey {
                space: "ws".into(),
                path: "d.md".into(),
            },
            Some("one\ntwo\n".as_bytes()),
        )
        .unwrap();
    (mgr, s)
}

fn join(s: &Session, pid: &str) -> ConnId {
    s.join(
        Participant::new(
            pid,
            pid.to_uppercase(),
            "#123456",
            ParticipantKind::Person,
            "tab",
        ),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn kinds(evs: &[ChangeEvent]) -> Vec<(u64, ChangeKind)> {
    evs.iter().map(|e| (e.version, e.kind)).collect()
}

#[test]
fn undo_and_redo_events_carry_their_kind_live_and_after_a_restart() {
    let dir = std::env::temp_dir().join(format!("k2mb-change-kind-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _guard = TmpDir(dir.clone());
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
    });
    let (mgr, s) = open(&dir, host.clone());
    let ann = join(&s, "ann");
    let rx = s.subscribe(ann).unwrap();
    let v0 = s.read(ann).unwrap().version;
    s.edit(
        ann,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: 4,
                end: 4,
                insert: "mine\n".into(),
            }],
        }],
        None,
    )
    .unwrap();
    let undone: UndoOutcome = s.undo(ann, 1).unwrap();
    assert!(format!("{undone:?}").starts_with("Applied"), "{undone:?}");
    let redone = s.redo(ann, 1).unwrap();
    assert!(format!("{redone:?}").starts_with("Applied"), "{redone:?}");
    let want = vec![
        (v0 + 1, ChangeKind::Edit),
        (v0 + 2, ChangeKind::Undo),
        (v0 + 3, ChangeKind::Redo),
    ];
    let live: Vec<ChangeEvent> = rx.try_iter().collect();
    assert_eq!(kinds(&live), want, "live: {live:?}");
    let since = s.events_since(ann, v0).unwrap();
    assert_eq!(kinds(&since), want, "events_since");

    // after a restart: from the store's change records
    mgr.release().unwrap();
    drop(s);
    let (mgr, s) = open(&dir, host);
    let ann = join(&s, "ann");
    let since = s.events_since(ann, v0).unwrap();
    assert_eq!(kinds(&since), want, "events_since after a restart");
    mgr.release().unwrap();
}
