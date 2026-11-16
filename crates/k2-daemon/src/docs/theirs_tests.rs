//! "Use their version": the save is kept as it was on disk (not the kit's
//! merge), and the kit's replace makes the doc that text as one change the
//! person's undo reverses.

use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, Config, EditOptions, Host, Participant, ParticipantKind, Rights, SessionManager, Stay, TextFilePack, UndoOutcome,
    WriteOptions,
};
use k2_mpc_base::store::DocStore;
use k2_mpc_base::text::Hunk;

use super::*;

const ROOT: &str = TextFilePack::ROOT;

struct Full;

impl Host for Full {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        1_700_000_000_000
    }
    fn on_event(&self, _: &AuditEvent) {}
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
fn their_save_is_kept_as_saved_and_used_as_one_undoable_change() {
    let dir = Dir(std::env::temp_dir().join(format!("k2-docs-theirs-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(dir.0.join("ws")).expect("dir");
    let file = dir.0.join("ws/notes.md");
    std::fs::write(&file, "one\ntwo\n").expect("file");
    let mut config = Config::default();
    config.edit_rate_per_s = 1_000_000;
    config.edit_burst = 1_000_000;
    let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).expect("store"));
    let mgr = SessionManager::new(store, Arc::new(Full), config);
    let key = DocKey { space: "theirs-ws".into(), path: "notes.md".into() };
    let s = mgr.open_file(Arc::new(TextFilePack::new("markdown", 4 << 20)), &key, &file, &dir.0.join("ws")).expect("open");
    let ada = s.join(Participant::new("person:ada", "Ada", "#7c3aed", ParticipantKind::Person, "tab"), Stay::Held).expect("join").conn;

    // Ada types; it isn't written back yet. Another program saves.
    let base = s.status().expect("status").version;
    s.edit_at_base(ada, ROOT, base, vec![Hunk { start: 0, end: 0, insert: "mine ".into() }], &EditOptions::new()).expect("typed");
    std::fs::write(&file, "one\ntwo\ntheirs\n").expect("their save");
    let r = ingest(&s, &key, &file).expect("taken in");
    let DiskCheck::Ingested { version } = r else { panic!("an outside save: {r:?}") };
    assert_eq!(get(&key), Some(Theirs { rev: version, text: "one\ntwo\ntheirs\n".into() }), "kept as it was on disk");
    let merged = text(&s, ada);
    assert_eq!(merged, "mine one\ntwo\ntheirs\n", "the kit merged it with Ada's unsaved edit");

    // Use their version: one change at the current version, and one undo puts the merge back.
    let theirs = get(&key).expect("kept");
    let w = s.replace_text_with_options(ada, ROOT, &theirs.text, &WriteOptions::new()).expect("use theirs");
    assert!(w.version > version, "a new change, after the save");
    assert_eq!(text(&s, ada), "one\ntwo\ntheirs\n");
    match s.undo(ada, 1).expect("undo") {
        UndoOutcome::Nothing | UndoOutcome::HistoryLost | UndoOutcome::ClearedByPurge => panic!("nothing undone"),
        _ => {}
    }
    assert_eq!(text(&s, ada), merged, "one undo step");

    // The kit's own write-back is an echo: nothing new is kept.
    s.flush(ada).expect("write back");
    let r = ingest(&s, &key, &file).expect("echo");
    assert!(!matches!(r, DiskCheck::Ingested { .. }), "{r:?}");
    assert_eq!(get(&key).map(|t| t.rev), Some(version));
    forget(&key);
    assert_eq!(get(&key), None);
}

#[test]
fn only_the_newest_saves_of_the_last_docs_are_kept() {
    let k = |n: usize| DocKey { space: "theirs-bound".into(), path: format!("d{n}.md") };
    remember(&k(0), 3, "a".into());
    remember(&k(0), 5, "b".into());
    assert_eq!(get(&k(0)), Some(Theirs { rev: 5, text: "b".into() }), "a doc's newest save only");
    for n in 1..=KEPT {
        remember(&k(n), 1, "x".into());
    }
    assert_eq!(get(&k(0)), None, "the oldest doc goes first");
    assert!(get(&k(KEPT)).is_some());
    remember(&k(KEPT + 1), 1, "y".repeat(TEXT_MAX + 1));
    assert_eq!(get(&k(KEPT + 1)), None, "too large to keep");
}
