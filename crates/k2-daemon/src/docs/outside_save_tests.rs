//! `outside_save.before_version` through the kit: a real outside save taken
//! in by the kit's own disk check, K2's audit hook ([`super::super::host::K2Host`])
//! naming the version from just before it, a socket subscribing after the
//! save (a tab that joined late) getting that frame, and the version read
//! back as the doc was before the save.

use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, Config, DiskCheck, EditOptions, Host, Participant, ParticipantKind, Rights, Session, SessionManager, Stay,
    TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionReason};
use k2_mpc_base::text::Hunk;

use super::*;

const ROOT: &str = TextFilePack::ROOT;

/// Every right; the audit hook is K2's own.
struct Full(super::super::host::K2Host);

impl Host for Full {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        super::super::host::now_ms()
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.0.on_event(ev)
    }
}

struct Dir(std::path::PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(s: &Session, c: k2_mpc_base::session::ConnId) -> String {
    s.read(c).expect("read").text_by_root.into_iter().find(|(r, _)| r == ROOT).expect("root").1
}

#[test]
fn a_tab_that_joins_after_an_outside_save_gets_the_version_from_before_it() {
    let dir = Dir(std::env::temp_dir().join(format!("k2-docs-before-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(dir.0.join("ws")).expect("dir");
    let file = dir.0.join("ws/notes.md");
    std::fs::write(&file, "one\ntwo\n").expect("file");
    let mut config = Config::default();
    config.edit_rate_per_s = 1_000_000;
    config.edit_burst = 1_000_000;
    let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).expect("store"));
    let host = Full(super::super::host::K2Host::new(Arc::downgrade(&store)));
    let mgr = SessionManager::new(store.clone(), Arc::new(host), config);
    let ws = dir.0.join("ws").canonicalize().expect("ws");
    let id = uuid::Uuid::new_v4().to_string();
    k2_core::db::shared()
        .lock()
        .execute("INSERT INTO projects (id, name, path) VALUES (?1, 'before-ws', ?2)", rusqlite::params![id, ws.to_string_lossy()])
        .expect("register the workspace");
    let key = DocKey { space: id, path: "notes.md".into() };
    let s = mgr.open_file(Arc::new(TextFilePack::new("markdown", 4 << 20)), &key, &file, &dir.0.join("ws")).expect("open");
    let ada = s.join(Participant::new("person:ada", "Ada", "#7c3aed", ParticipantKind::Person, "tab"), Stay::Held).expect("join").conn;
    // A tab open through the save (subscribed before it).
    let early = subscribe_replaying(&key, None, super::super::host::now_ms());
    assert!(early.rx.try_recv().is_err(), "no save yet");

    // Ada types (not written back yet); another program saves the file.
    let base = s.status().expect("status").version;
    s.edit_at_base(ada, ROOT, base, vec![Hunk { start: 0, end: 0, insert: "mine ".into() }], &EditOptions::new()).expect("typed");
    let before_text = text(&s, ada);
    std::fs::write(&file, "one\ntwo\ntheirs\n").expect("their save");
    let r = s.file_changed().expect("taken in");
    let DiskCheck::Ingested { version } = r else { panic!("an outside save: {r:?}") };
    assert_eq!(text(&s, ada), "mine one\ntwo\ntheirs\n", "merged");

    let before_of = |ev: HostEvent| match ev {
        HostEvent::OutsideSave { outcome: "ingested", rev, before_version, .. } => {
            assert_eq!(rev, Some(version), "the save's change");
            before_version.expect("the version from just before the save")
        }
        other => panic!("not an ingested outside_save: {other:?}"),
    };
    let live = before_of(early.rx.try_recv().expect("the open tab's outside_save"));

    // A tab that opens after the save: the same frame, replayed first.
    let late = subscribe_replaying(&key, None, super::super::host::now_ms());
    let ev = late.rx.try_recv().expect("the late tab's outside_save");
    let frame = ev.frame(|pid| k2_mpc_base::errors::Value::from(pid)).to_json();
    let replayed = before_of(ev);
    assert_eq!(replayed, live, "the same version on both tabs");
    assert!(frame.ends_with(&format!(r#""rev":{version},"before_version":{replayed}}}"#)), "{frame}");

    // It is the kit's version cut just before the save, and the late tab's
    // person reads the doc as it was then (`docs/show`'s kit call).
    let late_tab = s.join(Participant::new("person:ada", "Ada", "#7c3aed", ParticipantKind::Person, "late tab"), Stay::Held).expect("join").conn;
    let vs = s.versions(late_tab).expect("versions");
    let v = vs.iter().find(|v| v.id == replayed).expect("a stored version");
    assert_eq!((v.seq, v.reason), (version - 1, VersionReason::BeforeExternal), "{vs:?}");
    let shown = s.show(late_tab, replayed).expect("show").into_iter().find(|(r, _)| r == ROOT).expect("root").1;
    assert_eq!(shown, before_text, "the text from before their save");
    assert_eq!(shown, "mine one\ntwo\n");
    // A resume past the save already has it.
    assert!(subscribe_replaying(&key, Some(version), super::super::host::now_ms()).rx.try_recv().is_err());
    forget_saves(&key);
    super::super::theirs::forget(&key);
}
