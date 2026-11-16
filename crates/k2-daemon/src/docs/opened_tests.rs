//! The version an app opened, through the kit: K2's note on a live doc's
//! file, K2's audit hook for the kit's events, the refusals, and the one
//! fallback (a version that is gone is never asked for again).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, Config, Host, OpenedVia, Participant, ParticipantKind, Rights, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};

use super::*;

const ROOT: &str = TextFilePack::ROOT;

/// Every right, a clock the test moves, and K2's own audit hook.
struct Clocked {
    now: AtomicU64,
    k2: super::super::host::K2Host,
}

impl Host for Clocked {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.k2.on_event(ev)
    }
}

struct Dir(std::path::PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn person(pid: &str, via: &str) -> Participant {
    Participant::new(pid, pid, "#7c3aed", ParticipantKind::Person, via)
}

/// An app opened the file through K2 (the note), an agent's edit landed in
/// the file once the kit's hold was over, and the app then saved the copy it
/// opened with its own line: merged from the version it opened, both lines
/// are in the doc and in the file. The pin then follows the app's save.
#[test]
fn an_apps_save_merges_from_the_version_k2_noted() {
    let dir = Dir(std::env::temp_dir().join(format!("k2-docs-opened-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(dir.0.join("ws")).expect("dir");
    let file = dir.0.join("ws/notes.md");
    std::fs::write(&file, "one\ntwo\n").expect("file");
    let mut config = Config::default();
    config.edit_rate_per_s = 1_000_000;
    config.edit_burst = 1_000_000;
    let hold = config.opened_hold_ms;
    assert_eq!(hold, Config::default().opened_hold_ms, "K2 keeps the kit's hold");
    let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).expect("store"));
    let host = Arc::new(Clocked {
        now: AtomicU64::new(super::super::host::now_ms()),
        k2: super::super::host::K2Host::new(Arc::downgrade(&store)),
    });
    let mgr = SessionManager::new(store.clone(), host.clone(), config);
    let ws = dir.0.join("ws").canonicalize().expect("ws");
    let id = uuid::Uuid::new_v4().to_string();
    k2_core::db::shared()
        .lock()
        .execute("INSERT INTO projects (id, name, path) VALUES (?1, 'opened-ws', ?2)", rusqlite::params![id, ws.to_string_lossy()])
        .expect("register the workspace");
    let key = DocKey { space: id, path: "notes.md".into() };
    let s = mgr.open_file(Arc::new(TextFilePack::new("markdown", 4 << 20)), &key, &file, &dir.0.join("ws")).expect("open");
    assert_eq!(s.opened().expect("opened"), None, "no pin before K2 notes one");

    // K2 opens the file in another app: the file as it is now is pinned.
    let o = note_live(&s, person("person:owner", VIA_OPEN_IN_APP), OpenedAt::FileNow).expect("noted");
    assert_eq!((o.version, o.via), (s.status().expect("status").version, OpenedVia::FileNow));
    assert_eq!(s.opened().expect("opened").map(|p| p.version), Some(o.version), "pinned");

    // An agent adds a line; once the hold is over the kit writes it.
    let agent = s.join(person("agent:writer", "cli"), Stay::Held).expect("join").conn;
    let out = s.run_tool(agent, ROOT, &["append".to_owned()], b"agent\n").expect("append");
    assert_eq!(out.exit, 0);
    host.now.fetch_add(hold + 1, Ordering::SeqCst);
    assert!(s.flush(agent).expect("flush").written, "the kit wrote the file");
    assert_eq!(std::fs::read_to_string(&file).expect("read"), "one\ntwo\nagent\n");

    // The app saves the copy it opened, with its own line.
    std::fs::write(&file, "one\ntwo\nperson\n").expect("the app's save");
    s.file_changed().expect("taken in");
    let text = s.read(agent).expect("read").text_by_root.into_iter().find(|(r, _)| r == ROOT).expect("root").1;
    assert!(text.contains("agent\n") && text.contains("person\n"), "merged from the opened version: {text:?}");
    let p = s.opened().expect("opened").expect("a pin");
    assert_eq!((p.version, p.via), (s.status().expect("status").version, OpenedVia::OutsideSave), "the pin follows the save");

    super::super::host_events::forget_saves(&key);
    super::super::theirs::forget(&key);
}

/// A doc with no file (MPC-E182) through the kit: nothing is noted, and the
/// route answer is the kit's id with K2's words.
#[test]
fn a_doc_with_no_file_is_not_noted() {
    let dir = Dir(std::env::temp_dir().join(format!("k2-docs-opened-nofile-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&dir.0).expect("dir");
    let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).expect("store"));
    let host = Arc::new(Clocked {
        now: AtomicU64::new(super::super::host::now_ms()),
        k2: super::super::host::K2Host::new(Arc::downgrade(&store)),
    });
    let mgr = SessionManager::new(store, host, Config::default());
    let key = DocKey { space: "ws".into(), path: "mem.md".into() };
    let s = mgr.open(Arc::new(TextFilePack::new("markdown", 4 << 20)), &key, Some(b"x\n".as_slice())).expect("open");
    let e = note_live(&s, person("person:owner", VIA_OPEN_IN_APP), OpenedAt::FileNow).expect_err("no file");
    assert_eq!(e, SessionError::OpenedNoFile);
    assert_eq!(s.opened().expect("opened"), None, "nothing noted");
    let r = super::super::routes::session_error(&e);
    let body: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!((body["error"]["id"].as_str(), body["error"]["exit"].as_u64()), (Some("MPC-E182"), Some(2)), "{body}");
}

/// MPC-E181: a version that is gone is never asked for again; the file as
/// it is now is noted instead, once. Any other refusal, and a gone answer to
/// the file itself, stands as it is.
#[test]
fn a_gone_version_falls_back_to_the_file_once() {
    let mut asked = Vec::new();
    let r = note(OpenedAt::Version(7), |at| {
        asked.push(at);
        match at {
            OpenedAt::Version(v) => Err(SessionError::OpenedGone { version: v }),
            _ => Ok(12u64),
        }
    });
    assert_eq!(r, Ok(12));
    assert_eq!(asked, vec![OpenedAt::Version(7), OpenedAt::FileNow], "the same version is never asked twice");

    let mut asked = Vec::new();
    let r = note(OpenedAt::Version(9), |at| {
        asked.push(at);
        Err::<u64, _>(SessionError::OpenedUnknown { version: 9 })
    });
    assert_eq!(r, Err(SessionError::OpenedUnknown { version: 9 }));
    assert_eq!(asked, vec![OpenedAt::Version(9)], "an unknown version isn't the file");

    let mut asked = Vec::new();
    let r = note(OpenedAt::FileNow, |at| {
        asked.push(at);
        Err::<u64, _>(SessionError::OpenedGone { version: 3 })
    });
    assert_eq!(r, Err(SessionError::OpenedGone { version: 3 }));
    assert_eq!(asked, vec![OpenedAt::FileNow], "no loop on the file itself");
}

/// The kit's opened events reach their own Access Audit outcomes: the doc
/// and the version, and why a pin went. Metadata only.
#[test]
fn the_opened_events_have_their_own_audit_outcomes() {
    let key = DocKey { space: "proj-123".into(), path: "notes/a.md".into() };
    assert_eq!(super::super::host::opened_noted_outcome(&key, 42), "proj-123:notes/a.md opened_in_app version=42");
    assert_eq!(
        super::super::host::opened_dropped_outcome(&key, 42, "not_held"),
        "proj-123:notes/a.md opened_in_app_dropped version=42 why=not_held"
    );
}
