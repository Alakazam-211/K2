//! The edit path's runtime check (the canary, v0.9.3) in a session: what it reports
//! ([`AuditEvent::EditPathMismatch`], [`AuditEvent::TextCacheMismatch`]: metadata only),
//! that the result served is always right, and that the doc stays on the safe path until it
//! is reloaded. The divergence is planted (test builds only): the fast path's hunks front to
//! back, or a wrong cached text.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::differential::{with_side, Path, Side};
use crate::doc::Edit;
use crate::store::{DocKey, DocStore};
use crate::text::diff::Hunk;

struct H {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

struct Fx {
    dir: std::path::PathBuf,
    host: Arc<H>,
    mgr: SessionManager,
    key: DocKey,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    let dir = std::env::temp_dir().join(format!("k2mb-check-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
        events: Mutex::new(Vec::new()),
    });
    let mgr = mgr(&dir, &host);
    Fx {
        dir,
        host,
        mgr,
        key: DocKey {
            space: "ws".into(),
            path: "a.md".into(),
        },
    }
}

fn mgr(dir: &std::path::Path, host: &Arc<H>) -> SessionManager {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let c = Config {
        edit_check_every: 1,
        ..Config::default()
    };
    SessionManager::new(store, host.clone(), c)
}

impl Fx {
    fn open(&self) -> (Arc<Session>, ConnId) {
        let s = self
            .mgr
            .open(Arc::new(MD), &self.key, Some(b"one\ntwo\nthree\n"))
            .unwrap();
        let p = Participant::new("bot", "bot", "#123456", ParticipantKind::Agent, "cli");
        let c = s.join(p, Stay::Held).unwrap().conn;
        (s, c)
    }

    fn mismatches(&self) -> Vec<AuditEvent> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    AuditEvent::EditPathMismatch { .. } | AuditEvent::TextCacheMismatch { .. }
                )
            })
            .cloned()
            .collect()
    }
}

/// An edit inserting at two places (where front to back and back to front differ).
fn two_inserts(n: u32) -> Vec<Edit> {
    vec![Edit::Text {
        root: ROOT.into(),
        hunks: vec![
            Hunk {
                start: 0,
                end: 0,
                insert: format!("a{n}\n"),
            },
            Hunk {
                start: 4,
                end: 4,
                insert: format!("b{n}\n"),
            },
        ],
    }]
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root[0].1.clone()
}

#[test]
fn a_planted_edit_path_divergence_is_reported_and_never_served() {
    let f = fx("path");
    let mut side = Side::new(Path::MainDoc).planting_divergence();
    with_side(&mut side, || {
        let (s, c) = f.open();
        let r = s.edit(c, two_inserts(0), None).unwrap();
        // reported: the doc, the client, the change; nothing of the text
        let m = f.mismatches();
        assert_eq!(m.len(), 1, "{m:?}");
        let AuditEvent::EditPathMismatch { key, change, .. } = &m[0] else {
            panic!("{m:?}")
        };
        assert_eq!((key, *change), (&f.key, r.version));
        // served right: the text is the edit's
        assert_eq!(text(&s, c), "a0\none\nb0\ntwo\nthree\n");
        // the doc stays on the throwaway path: later sampled edits find nothing
        s.edit(c, two_inserts(1), None).unwrap();
        s.edit(c, two_inserts(2), None).unwrap();
        assert_eq!(f.mismatches().len(), 1);
        // until reloaded: a restart, and the planted divergence is caught again
        let want = text(&s, c);
        f.mgr.release().unwrap();
        let m2 = mgr(&f.dir, &f.host);
        let s = m2.open(Arc::new(MD), &f.key, None).unwrap();
        let p = Participant::new("bot", "bot", "#123456", ParticipantKind::Agent, "cli");
        let c = s.join(p, Stay::Held).unwrap().conn;
        assert_eq!(text(&s, c), want);
        s.edit(c, two_inserts(3), None).unwrap();
        assert_eq!(f.mismatches().len(), 2);
        m2.release().unwrap();
    });
}

#[test]
fn a_wrong_cached_text_is_reported_and_never_used() {
    let f = fx("cache");
    let (s, c) = f.open();
    s.edit(c, two_inserts(0), None).unwrap();
    let before = text(&s, c);
    // the engine's cache says something else: an edit planned against it would land wrong
    s.corrupt_text_cache_for_tests(ROOT, "zz\nzz\nzz\nzz\nzz\nzz\nzz\n")
        .unwrap();
    let r = s.edit(c, two_inserts(1), None).unwrap();
    let m = f.mismatches();
    assert_eq!(m.len(), 1, "{m:?}");
    let AuditEvent::TextCacheMismatch { key, root, change } = &m[0] else {
        panic!("{m:?}")
    };
    assert_eq!((key, root.as_str(), *change), (&f.key, ROOT, r.version));
    // the edit was planned against the fresh text
    let want = crate::text::diff::apply(
        &before,
        &[
            Hunk {
                start: 0,
                end: 0,
                insert: "a1\n".into(),
            },
            Hunk {
                start: 4,
                end: 4,
                insert: "b1\n".into(),
            },
        ],
    );
    assert_eq!(text(&s, c), want);
    // the cache stays off: corrupting it again changes nothing, and nothing more is reported
    s.corrupt_text_cache_for_tests(ROOT, "zz\n").unwrap();
    s.edit(c, two_inserts(2), None).unwrap();
    assert_eq!(f.mismatches().len(), 1);
    assert!(text(&s, c).starts_with("a2\n"));
}
