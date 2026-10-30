//! When an outside save removes someone's text (v0.9.7.2): the person who wrote it through the
//! kit is told what went, and the note carries their own text so they can paste it back.
//!
//! The merge doesn't change: these tests are about the note. "May have" is in every sentence,
//! because a moved line reads as removed. No single call restores the text and keeps the saver's
//! change; copying it back is the way, and the end-to-end tests here run exactly that. The exact
//! strings are pinned here and listed in `docs/evidence/v0.9.7.2-outside-removal.md`.
//!
//! `removal_cost_on_a_whole_document_delete` is a measurement (ignored; release):
//! `cargo test -p k2-mpc-base --release --test outside_removal_notes -- --ignored --nocapture`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::{self, NoteMutant};
use k2_mpc_base::doc::Edit;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, EditOptions, Host, NoteKind, Notes, Overwrite, Participant,
    ParticipantKind, PurgeRequest, PurgeScope, Rights, RosterEvent, Session, SessionManager, Stay,
    TextFilePack, OVERWRITE_CELLS_MAX, OVERWRITE_DOC_TEXT_MAX, OVERWRITE_PENDING_MAX,
    OVERWRITE_TEXT_MAX,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;

const HOUR: u64 = 60 * 60_000;

struct H {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for H {
    fn rights(&self, p: &Participant, _: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&p.pid)
            .copied()
            .unwrap_or_else(Rights::full)
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct Fx {
    dir: PathBuf,
    file: PathBuf,
    host: Arc<H>,
    mgr: SessionManager,
    key: DocKey,
    s: Arc<Session>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

static NEXT: AtomicU64 = AtomicU64::new(0);

fn fx(initial: &str) -> Fx {
    fx_with(initial, Config::default())
}

fn fx_with(initial: &str, config: Config) -> Fx {
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("k2mb-rmnotes-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let file = dir.join("ws/notes.md");
    std::fs::write(&file, initial).unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
        events: Mutex::new(Vec::new()),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), config);
    let key = DocKey {
        space: "ws".into(),
        path: "notes.md".into(),
    };
    let s = open(&mgr, &key, &file, &dir);
    Fx {
        dir,
        file,
        host,
        mgr,
        key,
        s,
    }
}

fn open(mgr: &SessionManager, key: &DocKey, file: &Path, dir: &Path) -> Arc<Session> {
    mgr.open_file(
        Arc::new(TextFilePack::new("markdown", 4 << 20)),
        key,
        file,
        &dir.join("ws"),
    )
    .unwrap()
}

impl Fx {
    fn join(&self, pid: &str, kind: ParticipantKind) -> ConnId {
        self.s
            .join(Participant::new(pid, pid, "#1", kind, "cli"), Stay::Held)
            .unwrap()
            .conn
    }
    fn agent(&self, pid: &str) -> ConnId {
        self.join(pid, ParticipantKind::Agent)
    }
    fn person(&self, pid: &str) -> ConnId {
        self.join(pid, ParticipantKind::Person)
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    /// `lines` inserted before line `at` by `c`, through a verb.
    fn insert(&self, c: ConnId, at: usize, lines: &str) -> Notes {
        let argv: Vec<String> = vec!["insert".to_owned(), "--line".to_owned(), at.to_string()];
        self.s
            .run_tool(c, TextFilePack::ROOT, &argv, lines.as_bytes())
            .unwrap()
            .notes
    }
    /// The kit writes the doc back, then an outside program saves `text`.
    fn save(&self, c: ConnId, text: &str) {
        self.s.flush(c).unwrap();
        self.advance(10_000);
        std::fs::write(&self.file, text).unwrap();
        self.s.file_changed().unwrap();
    }
    /// `c`'s next read: what its notes say.
    fn cat(&self, c: ConnId) -> Notes {
        self.s
            .run_tool(c, TextFilePack::ROOT, &["cat".to_owned()], b"")
            .unwrap()
            .notes
    }
    fn read_version(&self, c: ConnId, v: u64) -> String {
        self.s.show(c, v).unwrap()[0].1.clone()
    }
    fn text(&self, c: ConnId) -> String {
        self.s.read(c).unwrap().text_by_root[0].1.clone()
    }
    fn overrides(&self) -> Vec<AuditEvent> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, AuditEvent::OutsideOverride { .. }))
            .cloned()
            .collect()
    }
    fn set_rights(&self, pid: &str, r: Rights) {
        self.host.rights.lock().unwrap().insert(pid.to_owned(), r);
    }
    /// An ordinary edit: `text` appended to the doc.
    fn paste(&self, c: ConnId, text: &str) {
        self.advance(1_000); // an ordinary edit, not a burst
        let live = self.text(c);
        self.s
            .edit(
                c,
                vec![Edit::Text {
                    root: TextFilePack::ROOT.to_owned(),
                    hunks: vec![Hunk {
                        start: live.len(),
                        end: live.len(),
                        insert: text.to_owned(),
                    }],
                }],
                None,
            )
            .unwrap();
    }
    /// The session unloads: nobody is in it, and only the pool holds it. Then it opens again.
    fn unload_and_reopen(&mut self, after_ms: u64) {
        let other = other_doc(self);
        drop(std::mem::replace(&mut self.s, other));
        self.advance(after_ms.max(Config::default().idle_unload_ms + 1));
        // this session, and any stand-in from an earlier unload
        assert!(self.mgr.unload_idle().unwrap() >= 1, "the session unloaded");
        self.s = open(&self.mgr, &self.key, &self.file, &self.dir);
    }
}

/// A second doc's session, standing in for the fixture's while its session unloads.
fn other_doc(f: &Fx) -> Arc<Session> {
    let key = DocKey {
        space: "ws".into(),
        path: format!("other-{}.md", NEXT.fetch_add(1, Ordering::SeqCst)),
    };
    let file = f.dir.join("ws").join(&key.path);
    std::fs::write(&file, "x\n").unwrap();
    open(&f.mgr, &key, &file, &f.dir)
}

fn overwrites(n: &Notes) -> Vec<Overwrite> {
    n.notes
        .iter()
        .filter(|x| x.kind == NoteKind::Overwritten)
        .map(|x| x.overwrite.clone().expect("an Overwritten note says what"))
        .collect()
}

fn only(n: &Notes) -> Overwrite {
    let v = overwrites(n);
    assert_eq!(v.len(), 1, "{n:?}");
    v.into_iter().next().unwrap()
}

/// What an agent has: the note's line on stderr. Its `It read "…"`, unescaped.
fn text_in_line(line: &str) -> String {
    let at = line.find("It read \"").expect("the note carries the text") + "It read \"".len();
    let mut out = String::new();
    let mut it = line[at..].chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return out,
            '\\' => match it.next().unwrap() {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                c => out.push(c),
            },
            c => out.push(c),
        }
    }
    panic!("no closing quote: {line}");
}

const V0: &str = "intro\nmiddle\nend\n";
const LINE: &str = "the agent's line\n";
const SAVER: &str = "edited in the editor\n";

/// The probe's scenario: `c` adds a line and it reaches the file, then an editor holding the old
/// text saves it plus an edit of its own.
fn stale_save(f: &Fx, c: ConnId) {
    f.insert(c, 2, LINE);
    f.save(c, &format!("{V0}{SAVER}"));
}

/// K2's requirement, and the first test: what went, which version holds it, for an agent and a
/// person, and the named version really holds it.
#[test]
fn a_stale_save_tells_the_agent_what_went_and_which_version_holds_it() {
    let f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    assert!(!f.text(agent).contains(LINE), "the merge is what it was");
    let notes = f.cat(agent);
    let o = only(&notes);
    assert_eq!(o.rule, "removed_text");
    assert_eq!(o.region, "L2");
    assert_eq!(o.line, Some(2));
    assert_eq!(o.yours, LINE);
    let v = o.version.expect("a version holds it");
    assert!(f.read_version(agent, v).contains(LINE));
    assert_eq!(
        notes.to_string().trim_end(),
        format!(
            "note: An outside save may have removed your text (L2). Version {v} holds it. \
             Copy it back from there: restoring that version replaces the outside save's change \
             and everything since. It read \"the agent's line\\n\"."
        )
    );
    assert!(overwrites(&f.cat(agent)).is_empty(), "told once");

    // a person's acknowledged typing, through the live path
    let f = fx(V0);
    let sam = f.person("sam");
    type_live(&f, sam, "sam's typing\n");
    f.save(sam, &format!("{V0}{SAVER}"));
    assert!(!f.text(sam).contains("sam's typing"));
    let o = only(&f.s.take_notes(sam).unwrap());
    assert_eq!(o.yours, "sam's typing\n");
    assert!(f
        .read_version(sam, o.version.unwrap())
        .contains("sam's typing"));
}

/// A person's typing at the start of line 2, acknowledged.
fn type_live(f: &Fx, c: ConnId, text: &str) {
    let at = "intro\n".len();
    let base = f.s.status().unwrap().version;
    f.s.edit_at_base(
        c,
        TextFilePack::ROOT,
        base,
        vec![Hunk {
            start: at,
            end: at,
            insert: text.to_owned(),
        }],
        &EditOptions::new(),
    )
    .unwrap();
    assert!(f.text(c).contains(text), "acknowledged");
}

/// The way back, end to end, with the note's text alone: the author writes it back with an
/// ordinary edit, and ends with their text and the saver's. An agent reads the line it was given
/// on stderr; a person's client has the event.
#[test]
fn the_author_pastes_the_notes_text_back_and_keeps_the_savers_change() {
    let f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    let line = f.cat(agent).to_string();
    f.paste(agent, &text_in_line(&line));
    let now = f.text(agent);
    assert!(now.contains(LINE), "{now:?}");
    assert!(now.contains(SAVER), "{now:?}");

    let f = fx(V0);
    let sam = f.person("sam");
    type_live(&f, sam, "sam's typing\n");
    f.save(sam, &format!("{V0}{SAVER}"));
    let rx = f.s.subscribe_roster(sam).unwrap();
    let pushed = match rx.try_recv().unwrap() {
        RosterEvent::Snapshot {
            mut overwritten, ..
        } => overwritten.remove(0),
        e => panic!("{e:?}"),
    };
    f.paste(sam, &pushed.yours);
    let now = f.text(sam);
    assert!(now.contains("sam's typing\n"), "{now:?}");
    assert!(now.contains(SAVER), "{now:?}");
}

/// The other way back replaces the saver's change: what the note says about it is true.
#[test]
fn restoring_the_version_replaces_the_savers_change() {
    let f = fx(V0);
    let c = f.agent("agent");
    stale_save(&f, c);
    let o = only(&f.cat(c));
    f.s.restore(c, o.version.unwrap()).unwrap();
    let now = f.text(c);
    assert!(now.contains(LINE), "{now:?}");
    assert!(!now.contains(SAVER), "{now:?}");
}

/// A cut note: what it carries pastes back, it says how much it doesn't, and the version has the
/// rest, for someone who can open it.
#[test]
fn a_cut_note_says_so_and_what_it_carries_pastes_back() {
    let long: String = (0..4000)
        .map(|i| format!("line {i} of a long paste\n"))
        .collect();
    assert!(long.len() > OVERWRITE_TEXT_MAX);
    let total = long.chars().count();

    // with history
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, &long);
    f.save(agent, &format!("{V0}{SAVER}"));
    let notes = f.cat(agent);
    let o = only(&notes);
    assert!(o.cut);
    assert!(o.yours.len() <= OVERWRITE_TEXT_MAX);
    assert!(long.starts_with(&o.yours));
    assert_eq!(
        o.yours.chars().count() + o.not_shown,
        total,
        "nothing unaccounted"
    );
    let v = o.version.unwrap();
    let line = notes.to_string();
    assert!(
        line.trim_end().ends_with(&format!(
            "…\". {} more characters not shown; version {v} has them.",
            o.not_shown
        )),
        "{}",
        &line[line.len() - 120..]
    );
    f.paste(agent, &o.yours);
    let now = f.text(agent);
    assert!(now.contains(&o.yours));
    assert!(now.contains(SAVER));
    assert!(
        f.read_version(agent, v).contains(&long),
        "the version has the rest"
    );

    // without history: never told to open what they can't
    let f = fx(V0);
    let sam = f.person("sam");
    f.set_rights("sam", Rights::editor().with_history(false));
    f.insert(sam, 2, &long);
    f.save(sam, &format!("{V0}{SAVER}"));
    let notes = f.s.take_notes(sam).unwrap();
    let o = only(&notes);
    assert_eq!(o.version, None);
    let r = o.restore_from.expect("for telling someone with history");
    assert!(f.s.show(sam, r).is_err(), "they can't open it");
    let line = notes.to_string();
    assert!(
        line.trim_end().ends_with(&format!(
            "…\". {} more characters not shown; someone with history access can restore it from \
             version {r}.",
            o.not_shown
        )),
        "{}",
        &line[line.len() - 120..]
    );
    assert!(!line.contains("Copy it back from there"), "not for them");
    f.paste(sam, &o.yours);
    assert!(f.text(sam).contains(SAVER));
}

/// Each person gets only their own characters from one removed block.
#[test]
fn a_block_by_alice_and_bob_gives_each_only_their_own_text() {
    let f = fx(V0);
    let alice = f.person("alice");
    let bob = f.person("bob");
    f.insert(alice, 2, "alice's line\n");
    f.insert(bob, 3, "bob's line\n");
    f.save(alice, V0);
    let a = only(&f.s.take_notes(alice).unwrap());
    let b = only(&f.s.take_notes(bob).unwrap());
    assert_eq!(a.yours, "alice's line\n");
    assert_eq!(b.yours, "bob's line\n");
    assert_eq!((a.region.as_str(), b.region.as_str()), ("L2", "L3"));

    // inside one line too
    let f = fx(V0);
    let alice = f.person("alice");
    let bob = f.person("bob");
    f.insert(alice, 2, "alpha beta gamma\n");
    let at = f.text(bob).find("beta").unwrap();
    f.s.edit(
        bob,
        vec![Edit::Text {
            root: TextFilePack::ROOT.to_owned(),
            hunks: vec![Hunk {
                start: at,
                end: at + 4,
                insert: "BETA".to_owned(),
            }],
        }],
        None,
    )
    .unwrap();
    f.save(alice, V0);
    let a = only(&f.s.take_notes(alice).unwrap());
    let b = only(&f.s.take_notes(bob).unwrap());
    assert_eq!(a.yours, "alpha  gamma\n");
    assert_eq!(b.yours, "BETA");
}

/// A line deleted between two that start alike is that line, whoever wrote its neighbours.
#[test]
fn a_line_deleted_between_lines_that_start_alike_is_its_authors_alone() {
    let f = fx(V0);
    let alice = f.person("alice");
    let bob = f.person("bob");
    f.insert(alice, 2, "- item one\n");
    f.insert(bob, 3, "- item two\n");
    let after = f.text(alice);
    f.save(alice, &after.replace("- item one\n", ""));
    let a = only(&f.s.take_notes(alice).unwrap());
    assert_eq!(a.yours, "- item one\n");
    assert!(overwrites(&f.s.take_notes(bob).unwrap()).is_empty());
}

#[test]
fn a_stale_save_pushes_overwritten_to_the_author_only() {
    let f = fx(V0);
    let agent = f.agent("agent");
    let sam = f.person("sam");
    let to_agent = f.s.subscribe_roster(agent).unwrap();
    let to_sam = f.s.subscribe_roster(sam).unwrap();
    stale_save(&f, agent);
    f.advance(1_000);
    f.s.flush_roster().unwrap();
    let pushed = |rx: &std::sync::mpsc::Receiver<RosterEvent>| -> Vec<Overwrite> {
        rx.try_iter()
            .filter_map(|e| match e {
                RosterEvent::Overwritten { overwrite, .. } => Some(overwrite),
                _ => None,
            })
            .collect()
    };
    let got = pushed(&to_agent);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].yours, LINE);
    assert!(got[0].version.is_some());
    assert!(
        pushed(&to_sam).is_empty(),
        "only its author's subscriptions"
    );
    // a late subscription finds it in its snapshot: a push doesn't consume it
    let late = f.s.subscribe_roster(agent).unwrap();
    match late.try_recv().unwrap() {
        RosterEvent::Snapshot { overwritten, .. } => {
            assert_eq!(overwritten.len(), 1);
            assert_eq!(
                overwritten[0].change, got[0].change,
                "dedupe on (change, row)"
            );
            assert_eq!(overwritten[0].row, got[0].row);
        }
        e => panic!("{e:?}"),
    }
    let sam_late = f.s.subscribe_roster(sam).unwrap();
    match sam_late.try_recv().unwrap() {
        RosterEvent::Snapshot { overwritten, .. } => assert!(overwritten.is_empty()),
        e => panic!("{e:?}"),
    }
}

#[test]
fn old_text_past_the_window_after_a_later_outside_save_gets_no_note() {
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, LINE);
    // an outside save later than the line, which keeps it
    let with_line = f.text(agent);
    f.save(agent, &format!("{with_line}from outside\n"));
    f.cat(agent);
    f.advance(25 * HOUR);
    f.host.events.lock().unwrap().clear();
    f.save(agent, &format!("{V0}from outside\n"));
    assert!(!f.text(agent).contains(LINE));
    assert!(overwrites(&f.cat(agent)).is_empty());
    assert!(f.overrides().is_empty(), "{:?}", f.overrides());
}

#[test]
fn text_within_the_window_gets_a_note_after_a_later_outside_save() {
    // X holds an old buffer, Y saves fresh, X saves: "since the last save" alone misses it
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, LINE);
    let with_line = f.text(agent);
    f.save(agent, &format!("{with_line}fresh save\n"));
    f.cat(agent);
    f.advance(HOUR);
    f.save(agent, &format!("{V0}stale save\n"));
    assert_eq!(only(&f.cat(agent)).yours, LINE);
}

#[test]
fn text_since_the_last_outside_save_gets_a_note_past_the_window() {
    // an editor left open for two days, with no outside save since the line
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, LINE);
    f.s.flush(agent).unwrap();
    f.advance(48 * HOUR);
    f.save(agent, &format!("{V0}{SAVER}"));
    assert_eq!(only(&f.cat(agent)).yours, LINE);
}

#[test]
fn the_window_is_configurable_and_zero_means_only_since_the_last_save() {
    let mut config = Config::default();
    config.outside_note_window_ms = 0;
    let f = fx_with(V0, config);
    let agent = f.agent("agent");
    f.insert(agent, 2, LINE);
    let with_line = f.text(agent);
    f.save(agent, &format!("{with_line}fresh save\n"));
    f.cat(agent);
    f.save(agent, &format!("{V0}stale save\n"));
    assert!(overwrites(&f.cat(agent)).is_empty());
}

#[test]
fn whitespace_only_changes_get_no_note() {
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, "kept line   \n\n\t\n");
    let after = f.text(agent);
    // an editor stripping trailing whitespace and blank lines
    f.save(agent, &after.replace("kept line   \n\n\t\n", "kept line\n"));
    assert_eq!(f.text(agent), "intro\nkept line\nmiddle\nend\n");
    assert!(overwrites(&f.cat(agent)).is_empty());
    assert!(f.overrides().is_empty(), "{:?}", f.overrides());
}

#[test]
fn adding_to_a_line_gets_no_note() {
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, "alpha\n");
    let after = f.text(agent);
    f.save(agent, &after.replace("alpha\n", "alpha and more\n"));
    assert!(overwrites(&f.cat(agent)).is_empty());
}

#[test]
fn a_replaced_line_reports_yours_and_the_files() {
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, "alpha beta\n");
    let after = f.text(agent);
    f.save(agent, &after.replace("alpha beta", "alpha gamma"));
    let notes = f.cat(agent);
    let o = only(&notes);
    assert_eq!(o.rule, "replaced_text");
    assert_eq!(o.yours, "alpha beta\n");
    assert_eq!(o.file, "alpha gamma");
    let v = o.version.unwrap();
    assert_eq!(
        notes.to_string().trim_end(),
        format!(
            "note: An outside save may have replaced your text (L2). Version {v} holds it. \
             Copy it back from there: restoring that version replaces the outside save's change \
             and everything since. It read \"alpha beta\\n\"; the file now has \"alpha gamma\"."
        )
    );
}

#[test]
fn a_range_of_lines_is_one_place_and_says_may_have() {
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, "one\ntwo\nthree\n");
    f.save(agent, V0);
    let notes = f.cat(agent);
    let o = only(&notes);
    assert_eq!(o.region, "L2–L4");
    assert_eq!(o.yours, "one\ntwo\nthree\n");
    let s = notes.to_string();
    assert!(
        s.starts_with("note: An outside save may have removed your text (L2–L4). Version "),
        "{s}"
    );
    assert_eq!(text_in_line(&s), "one\ntwo\nthree\n", "exactly recoverable");
}

#[test]
fn text_with_quotes_and_backslashes_comes_back_exactly() {
    let f = fx(V0);
    let agent = f.agent("agent");
    let tricky = "say \"hi\" \\ and\ttab\n";
    f.insert(agent, 2, tricky);
    f.save(agent, V0);
    assert_eq!(text_in_line(&f.cat(agent).to_string()), tricky);
}

#[test]
fn a_moved_line_is_reported_with_may_have() {
    // the kit can't tell this from a removal, so the words mustn't say it is gone
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, "moved line\n");
    f.save(agent, "intro\nmiddle\nend\nmoved line\n");
    assert!(f.text(agent).contains("moved line"));
    let notes = f.cat(agent);
    assert_eq!(only(&notes).yours, "moved line\n");
    assert!(notes.to_string().contains("may have removed"), "{notes}");
}

/// The person an assistant acted for gets the text made on their behalf, nothing more; and the
/// assistant gets its own, not the person's.
#[test]
fn an_assistants_text_tells_the_person_it_acted_for() {
    let f = fx(V0);
    let sam = f.person("sam");
    let aide =
        f.s.join(
            Participant::new("aide", "Aide", "#3", ParticipantKind::Agent, "cli")
                .on_behalf_of("sam"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let other = f.person("other");
    f.insert(aide, 2, "written for sam\n");
    f.insert(other, 3, "other's line\n");
    f.save(sam, V0);
    let for_aide = only(&f.cat(aide));
    assert_eq!(for_aide.yours, "written for sam\n");
    let for_sam = only(&f.cat(sam));
    assert_eq!(for_sam.yours, "written for sam\n", "nothing more");
    assert_eq!(only(&f.cat(other)).yours, "other's line\n");
}

/// Whoever put text back is told where, not given it: it isn't theirs.
#[test]
fn text_put_back_by_a_revert_tells_who_put_it_back_without_the_text() {
    let f = fx(V0);
    let a = f.agent("a");
    let b = f.agent("b");
    f.set_rights("b", Rights::full().with_owner(true));
    let text = "wrote then deleted\n";
    f.insert(a, 2, text);
    let at = f.text(a).find("wrote").unwrap();
    let deleted =
        f.s.edit(
            a,
            vec![Edit::Text {
                root: TextFilePack::ROOT.to_owned(),
                hunks: vec![Hunk {
                    start: at,
                    end: at + text.len(),
                    insert: String::new(),
                }],
            }],
            None,
        )
        .unwrap();
    f.s.revert(b, deleted.version, false).unwrap();
    assert!(f.text(a).contains("wrote then deleted"));
    f.save(a, V0);
    assert_eq!(only(&f.cat(a)).yours, text);
    let notes = f.cat(b);
    let o = only(&notes);
    assert_eq!(o.yours, "");
    assert_eq!(o.not_shown, text.chars().count());
    assert!(!notes.to_string().contains("wrote then deleted"), "{notes}");
}

/// The biggest limit, pinned: the rule covers **authors who wrote through the kit**. Text that
/// came in through a file save has no author to tell. An audit row names `kit:external`; nobody
/// gets a note.
#[test]
fn kit_external_text_gets_no_note() {
    let f = fx(V0);
    let agent = f.agent("agent");
    let sam = f.person("sam");
    f.save(agent, &format!("{V0}a file tool's line\n"));
    f.cat(agent);
    f.host.events.lock().unwrap().clear();
    f.save(agent, V0);
    assert!(overwrites(&f.cat(agent)).is_empty());
    assert!(overwrites(&f.cat(sam)).is_empty());
    let rows = f.overrides();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        matches!(&rows[0], AuditEvent::OutsideOverride { by: Some(b), .. } if b == "kit:external"),
        "{rows:?}"
    );
}

/// Text the file never had isn't removed by the file: the merge keeps it, and there's nothing to say.
#[test]
fn text_not_yet_written_back_is_kept_and_gets_no_note() {
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, LINE);
    // no write-back: the editor saves over a file that never held the line
    f.advance(1);
    std::fs::write(&f.file, format!("{V0}{SAVER}")).unwrap();
    f.s.file_changed().unwrap();
    let now = f.text(agent);
    assert!(now.contains(LINE), "{now:?}");
    assert!(now.contains(SAVER), "{now:?}");
    assert!(overwrites(&f.cat(agent)).is_empty());
}

/// A save of exactly an older write of the kit's is an echo: nothing is removed.
#[test]
fn an_echo_of_the_kits_own_write_gets_no_note() {
    let f = fx(V0);
    let agent = f.agent("agent");
    f.insert(agent, 2, "first\n");
    f.s.flush(agent).unwrap();
    let older = std::fs::read_to_string(&f.file).unwrap();
    f.advance(2_000);
    f.insert(agent, 2, "second\n");
    f.s.flush(agent).unwrap();
    f.advance(1_000);
    std::fs::write(&f.file, &older).unwrap();
    f.s.file_changed().unwrap();
    assert!(f.text(agent).contains("second"));
    assert!(overwrites(&f.cat(agent)).is_empty());
}

#[test]
fn a_rekeyed_doc_keeps_its_held_notes() {
    let mut f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    assert!(f.s.leave(agent).unwrap());
    let other = other_doc(&f);
    drop(std::mem::replace(&mut f.s, other));
    f.advance(Config::default().idle_unload_ms + 1);
    assert_eq!(f.mgr.unload_idle().unwrap(), 1);
    let new = DocKey {
        space: "ws".into(),
        path: "renamed.md".into(),
    };
    f.mgr.rekey(&f.key, new.clone()).unwrap();
    f.key = new;
    f.s = open(&f.mgr, &f.key, &f.file, &f.dir);
    let agent = f.agent("agent");
    assert_eq!(only(&f.s.take_notes(agent).unwrap()).yours, LINE);
}

/// The host's own edit calls return no notes, so they must not take them: the note waits for a
/// call that delivers it (the soak found the gap: e07c5b1e, 7a2e4281).
#[test]
fn a_hosts_edit_call_leaves_the_note_for_the_next_verb() {
    let f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    f.advance(1_000);
    f.s.edit_with_options(agent, TextFilePack::ROOT, &EditOptions::new(), |live| {
        Ok::<_, std::convert::Infallible>(format!("{live}typed in a tab\n"))
    })
    .unwrap();
    f.s.edit_with(agent, TextFilePack::ROOT, |live| {
        Ok::<_, std::convert::Infallible>(format!("{live}and more\n"))
    })
    .unwrap();
    f.s.replace_text(
        agent,
        TextFilePack::ROOT,
        &format!("{}last\n", f.text(agent)),
    )
    .unwrap();
    assert_eq!(
        only(&f.cat(agent)).yours,
        LINE,
        "still waiting, then delivered"
    );
}

#[test]
fn the_planted_edit_with_drain_is_caught() {
    let lost = differential::with_note_mutant(NoteMutant::EditWithDrains, || {
        let f = fx(V0);
        let agent = f.agent("agent");
        stale_save(&f, agent);
        f.advance(1_000);
        f.s.edit_with(agent, TextFilePack::ROOT, |live| {
            Ok::<_, std::convert::Infallible>(format!("{live}typed in a tab\n"))
        })
        .unwrap();
        overwrites(&f.cat(agent)).is_empty()
    });
    assert!(
        lost,
        "planted, the note is lost: the test above is what catches it"
    );
}

/// `write --base` returns its notes: it delivers them.
#[test]
fn write_base_delivers_the_note() {
    let f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    let v = f.s.status().unwrap().version;
    let body = format!("{}appended\n", f.text(agent));
    f.advance(1_000);
    let r =
        f.s.write_base_with(
            agent,
            TextFilePack::ROOT,
            v,
            &body,
            k2_mpc_base::session::ClaimPolicy::Ignore,
        )
        .unwrap();
    assert_eq!(only(&r.notes).yours, LINE);
    assert!(overwrites(&f.cat(agent)).is_empty(), "told once");
}

#[test]
fn imported_text_gets_no_note() {
    let f = fx(V0);
    let agent = f.agent("agent");
    f.save(agent, "intro\nend\n");
    assert!(overwrites(&f.cat(agent)).is_empty());
}

#[test]
fn no_read_no_note_no_count() {
    let f = fx(V0);
    let agent = f.agent("agent");
    let rx = f.s.subscribe_roster(agent).unwrap();
    f.insert(agent, 2, LINE);
    f.s.flush(agent).unwrap();
    f.set_rights("agent", Rights::none());
    f.advance(10_000);
    std::fs::write(&f.file, V0).unwrap();
    f.s.file_changed().unwrap();
    f.advance(1_000);
    f.s.flush_roster().unwrap();
    let any = rx
        .try_iter()
        .any(|e| matches!(e, RosterEvent::Overwritten { .. }));
    assert!(!any, "no push without read");
    assert!(f.s.take_notes(agent).is_err(), "no verb without read");
    // it read again: the note arrives
    f.set_rights("agent", Rights::full());
    let agent = f.agent("agent");
    assert_eq!(only(&f.s.take_notes(agent).unwrap()).yours, LINE);
}

/// `history` is its own grant: without it the push and the note name no version to open, and the
/// text is there to paste.
#[test]
fn an_author_without_history_is_named_no_version_and_gets_the_text() {
    let f = fx(V0);
    let sam = f.person("sam");
    f.set_rights("sam", Rights::editor().with_history(false));
    let rx = f.s.subscribe_roster(sam).unwrap();
    f.insert(sam, 2, "sam's line\n");
    f.save(sam, V0);
    f.advance(1_000);
    f.s.flush_roster().unwrap();
    let pushed: Vec<Overwrite> = rx
        .try_iter()
        .filter_map(|e| match e {
            RosterEvent::Overwritten { overwrite, .. } => Some(overwrite),
            _ => None,
        })
        .collect();
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].version, None, "they couldn't open it");
    assert!(pushed[0].no_history);
    let notes = f.s.take_notes(sam).unwrap();
    let o = only(&notes);
    assert_eq!(o.version, None);
    assert_eq!(o.yours, "sam's line\n");
    assert_eq!(
        notes.to_string().trim_end(),
        "note: An outside save may have removed your text (L2). It read \"sam's line\\n\"."
    );
    f.paste(sam, &o.yours);
    assert!(f.text(sam).contains("sam's line\n"));
}

#[test]
fn bounds_per_note_list_32_places_and_count_the_rest() {
    let n = 100usize;
    let f = fx(V0);
    let agent = f.agent("agent");
    let body: String = (0..n).map(|i| format!("kept {i}\nmine {i}\n")).collect();
    f.insert(agent, 2, &body);
    let after = f.text(agent);
    let saved: String = after
        .lines()
        .filter(|l| !l.starts_with("mine"))
        .map(|l| format!("{l}\n"))
        .collect();
    f.save(agent, &saved);
    let notes = f.cat(agent);
    let o = only(&notes);
    assert_eq!(o.also.len() + 1, OVERWRITE_CELLS_MAX);
    assert_eq!(o.more, n - OVERWRITE_CELLS_MAX);
    assert_eq!(o.cells(), n, "nothing dropped");
    let listed: usize = o.yours.chars().count()
        + o.also
            .iter()
            .map(|c| c.yours.chars().count())
            .sum::<usize>();
    let total: usize = (0..n).map(|i| format!("mine {i}\n").chars().count()).sum();
    assert_eq!(listed + o.not_shown, total, "every character accounted for");
    assert!(notes.to_string().contains("and 68 more places"), "{notes}");
}

/// `n` saves that each remove one of the agent's lines, nothing delivered in between.
fn saves_removing_a_line_each(f: &Fx, c: ConnId, n: usize) {
    let body: String = (0..n).map(|i| format!("line {i}\n")).collect();
    f.insert(c, 2, &body);
    let mut text = f.text(c);
    for i in 0..n {
        text = text.replace(&format!("line {i}\n"), "");
        f.save(c, &text);
    }
}

#[test]
fn bounds_roll_up_with_exact_counts() {
    let n = OVERWRITE_PENDING_MAX + 6;
    let f = fx(V0);
    let agent = f.agent("agent");
    saves_removing_a_line_each(&f, agent, n);
    let notes = f.s.take_notes(agent).unwrap();
    let got = overwrites(&notes);
    assert_eq!(got.len(), OVERWRITE_PENDING_MAX);
    let last = got.last().unwrap();
    assert!(last.is_rollup(), "{last:?}");
    assert_eq!(
        got.iter().map(Overwrite::cells).sum::<usize>(),
        n,
        "a note is never dropped silently"
    );
    assert_eq!(last.more, 7);
    let v = last.version.unwrap();
    assert_eq!(
        last.to_string(),
        format!(
            "An outside save may have removed or replaced your text in 7 more places. See \
             change {} in the log. Version {v} holds the latest of it.",
            last.change
        )
    );
}

#[test]
fn debug_and_audit_hold_no_content() {
    let f = fx(V0);
    let agent = f.agent("agent");
    let rx = f.s.subscribe_roster(agent).unwrap();
    f.insert(agent, 2, "topsecretvalue\n");
    let after = f.text(agent);
    f.save(
        agent,
        &after.replace("topsecretvalue", "topsecretreplacement"),
    );
    f.advance(1_000);
    f.s.flush_roster().unwrap();
    for e in rx.try_iter() {
        let s = format!("{e:?}");
        assert!(!s.contains("topsecret"), "{s}");
    }
    let notes = f.s.take_notes(agent).unwrap();
    let s = format!("{notes:?}");
    assert!(!s.contains("topsecret"), "{s}");
    assert!(
        notes.to_string().contains("topsecretvalue"),
        "the note says it"
    );
    let rows = f.overrides();
    assert_eq!(rows.len(), 1, "{rows:?}");
    for e in f.host.events.lock().unwrap().iter() {
        let s = format!("{e:?}");
        assert!(!s.contains("topsecret"), "{s}");
    }
}

// ----- held across an unload -----

/// K2's agents mostly run one verb at a time, so this is their normal case: the agent comes back
/// after the session unloaded, gets its text, and pastes it back.
#[test]
fn an_agent_returning_after_unload_gets_its_text_and_pastes_it_back() {
    let mut f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    assert!(f.s.leave(agent).unwrap());
    f.unload_and_reopen(0);
    let agent = f.agent("agent");
    let notes = f.cat(agent);
    let o = only(&notes);
    assert!(!o.withheld);
    assert_eq!(o.yours, LINE);
    assert!(f.read_version(agent, o.version.unwrap()).contains(LINE));
    f.paste(agent, &text_in_line(&notes.to_string()));
    let now = f.text(agent);
    assert!(now.contains(LINE), "{now:?}");
    assert!(now.contains(SAVER), "{now:?}");
}

#[test]
fn a_purge_while_held_takes_the_text() {
    let mut f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    assert!(f.s.leave(agent).unwrap());
    f.unload_and_reopen(0);
    // a purge reaches a doc only through a session, which has adopted its notes
    let owner = f.person("owner");
    f.set_rights("owner", Rights::full().with_owner(true));
    f.s.purge(owner, PurgeRequest::new(PurgeScope::History))
        .unwrap();
    let agent = f.agent("agent");
    let notes = f.s.take_notes(agent).unwrap();
    let o = only(&notes);
    assert!(o.withheld);
    assert_eq!((o.yours.as_str(), o.file.as_str()), ("", ""));
    assert_eq!((o.version, o.restore_from), (None, None));
    assert_eq!(
        notes.to_string().trim_end(),
        format!(
            "note: An outside save may have removed your text (L2). The text is no longer held \
             here. See change {} in the log.",
            o.change
        )
    );
}

#[test]
fn access_lost_while_held_delivers_nothing_until_it_returns() {
    let mut f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    assert!(f.s.leave(agent).unwrap());
    f.unload_and_reopen(0);
    f.set_rights("agent", Rights::none());
    let refused = f.s.join(
        Participant::new("agent", "agent", "#1", ParticipantKind::Agent, "cli"),
        Stay::Held,
    );
    if let Ok(j) = refused {
        assert!(
            f.s.take_notes(j.conn).is_err(),
            "no read, nothing delivered"
        );
        assert!(f.s.subscribe_roster(j.conn).is_err());
        let _ = f.s.leave(j.conn);
    }
    f.set_rights("agent", Rights::full());
    let agent = f.agent("agent");
    assert_eq!(only(&f.s.take_notes(agent).unwrap()).yours, LINE);
}

#[test]
fn the_text_is_held_for_24_hours_then_the_note_says_so() {
    let mut f = fx(V0);
    let agent = f.agent("agent");
    let sam = f.person("sam");
    f.set_rights("sam", Rights::editor().with_history(false));
    f.insert(agent, 2, LINE);
    f.insert(sam, 3, "sam's line\n");
    f.save(agent, &format!("{V0}{SAVER}"));
    assert!(f.s.leave(agent).unwrap());
    assert!(f.s.leave(sam).unwrap());
    // just inside: still there
    f.unload_and_reopen(23 * HOUR);
    let agent = f.agent("agent");
    assert_eq!(only(&f.cat(agent)).yours, LINE);
    assert!(f.s.leave(agent).unwrap());
    // past it: the fact, the version for someone who can open it, and who can help for someone
    // who can't
    f.unload_and_reopen(2 * HOUR);
    let sam = f.person("sam");
    let notes = f.s.take_notes(sam).unwrap();
    let o = only(&notes);
    assert!(o.withheld);
    assert_eq!(o.yours, "");
    let r = o.restore_from.unwrap();
    assert_eq!(
        notes.to_string().trim_end(),
        format!(
            "note: An outside save may have removed your text (L3). The text is no longer held \
             here. Someone with history access can restore it from version {r}."
        )
    );

    let mut f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    assert!(f.s.leave(agent).unwrap());
    f.unload_and_reopen(25 * HOUR);
    let agent = f.agent("agent");
    let notes = f.cat(agent);
    let o = only(&notes);
    assert!(o.withheld);
    let v = o.version.unwrap();
    assert_eq!(
        notes.to_string().trim_end(),
        format!(
            "note: An outside save may have removed your text (L2). The text is no longer held \
             here. Version {v} holds it. Copy it back from there: restoring that version replaces \
             the outside save's change and everything since."
        )
    );
    assert!(f.read_version(agent, v).contains(LINE));
}

/// One doc's waiting text is capped: the oldest go first, and the note says so.
#[test]
fn the_doc_total_cap_takes_the_oldest_text_and_says_so() {
    let block = |who: &str, i: usize| -> String {
        let mut s = format!("{who} block {i}\n");
        while s.len() < OVERWRITE_TEXT_MAX {
            s.push_str("0123456789 0123456789 0123456789 0123456789\n");
        }
        let mut end = OVERWRITE_TEXT_MAX;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        if !s.ends_with('\n') {
            s.pop();
            s.push('\n');
        }
        s
    };
    let f = fx(V0);
    let a = f.agent("a");
    let b = f.agent("b");
    let per = 34; // 68 x 16 KiB is past the 1 MiB cap, and each author is under their 64 notes
    let mut text = V0.to_owned();
    for i in 0..per {
        for (who, c) in [("a", a), ("b", b)] {
            let chunk = block(who, i);
            let live = f.text(c);
            f.paste(c, &chunk);
            text = live + &chunk;
        }
    }
    let mut left = text;
    for i in 0..per {
        for who in ["a", "b"] {
            left = left.replace(&block(who, i), "");
            f.save(a, &left);
        }
    }
    let notes_a = overwrites(&f.s.take_notes(a).unwrap());
    let notes_b = overwrites(&f.s.take_notes(b).unwrap());
    assert_eq!(notes_a.len() + notes_b.len(), 2 * per, "every fact stays");
    let total: usize = notes_a
        .iter()
        .chain(&notes_b)
        .map(|o| o.yours.len() + o.also.iter().map(|c| c.yours.len()).sum::<usize>())
        .sum();
    assert!(total <= OVERWRITE_DOC_TEXT_MAX, "{total}");
    assert!(notes_a.first().unwrap().withheld, "the oldest went");
    assert!(!notes_b.last().unwrap().withheld, "the newest stayed");
    assert!(notes_a
        .first()
        .unwrap()
        .to_string()
        .contains("The text is no longer held here."));
}

/// No restart persistence in this release: the note goes; the version and the audit row remain.
#[test]
fn restart_loses_notes_and_the_version_remains() {
    let f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    assert_eq!(f.overrides().len(), 1, "the audit row was written");
    f.mgr.release().unwrap();
    // the next process, on the same store
    let store = Arc::new(DocStore::open(f.dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, f.host.clone(), Config::default());
    let s = open(&mgr, &f.key, &f.file, &f.dir);
    let agent = s
        .join(
            Participant::new("agent", "agent", "#1", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let out = s
        .run_tool(agent, TextFilePack::ROOT, &["cat".to_owned()], b"")
        .unwrap();
    assert!(
        overwrites(&out.notes).is_empty(),
        "lost: the CHANGELOG says so"
    );
    let holding = s
        .versions(agent)
        .unwrap()
        .iter()
        .filter(|v| s.show(agent, v.id).unwrap()[0].1.contains(LINE))
        .count();
    assert!(holding > 0, "the version remains");
    drop(s);
    mgr.release().unwrap();
}

#[test]
fn purge_withholds_waiting_notes_in_a_loaded_session() {
    let f = fx(V0);
    let agent = f.agent("agent");
    let owner = f.person("owner");
    f.set_rights("owner", Rights::full().with_owner(true));
    stale_save(&f, agent);
    f.s.purge(owner, PurgeRequest::new(PurgeScope::History))
        .unwrap();
    let o = only(&f.s.take_notes(agent).unwrap());
    assert!(o.withheld);
    assert_eq!((o.yours.as_str(), o.version), ("", None));
}

#[test]
fn an_erased_participant_has_no_notes_left() {
    let f = fx(V0);
    let agent = f.agent("agent");
    let owner = f.person("owner");
    f.set_rights("owner", Rights::full().with_owner(true));
    stale_save(&f, agent);
    f.s.purge(
        owner,
        PurgeRequest::new(PurgeScope::Participant {
            pid: "agent".to_owned(),
            pseudonym: "erased-1".to_owned(),
        }),
    )
    .unwrap();
    let again = f.agent("agent");
    assert!(overwrites(&f.s.take_notes(again).unwrap()).is_empty());
}

#[test]
fn deleting_the_doc_forgets_held_notes() {
    let mut f = fx(V0);
    let agent = f.agent("agent");
    stale_save(&f, agent);
    assert!(f.s.leave(agent).unwrap());
    let other = other_doc(&f);
    drop(std::mem::replace(&mut f.s, other));
    f.advance(Config::default().idle_unload_ms + 1);
    assert_eq!(f.mgr.unload_idle().unwrap(), 1);
    f.mgr.delete_doc(&f.key).unwrap();
    f.s = open(&f.mgr, &f.key, &f.file, &f.dir);
    let agent = f.agent("agent");
    assert!(overwrites(&f.s.take_notes(agent).unwrap()).is_empty());
}

// ----- the oracle -----

/// A small deterministic generator (a failure prints its seed).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }
}

/// Every removal the rule covers is accounted for in its author's notes, as exactly the text it
/// removed, and nothing is reported that wasn't removed. Worked out apart from the kit's code:
/// each line is written whole and is unique, so the removed text is the lines not in the saved
/// text, and their author is in the line's name.
fn check_case(seed: u64) -> Result<(), String> {
    let mut r = Lcg(seed);
    let f = fx(V0);
    let names = ["a", "b", "c"];
    let conns: Vec<ConnId> = names.iter().map(|n| f.agent(n)).collect();
    let mut made = 0usize;
    let mut expect: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    // a verb delivers what's waiting, so an author's own later lines collect some of it
    let mut got: Vec<Vec<Overwrite>> = vec![Vec::new(); names.len()];
    for _round in 0..4 {
        // people write lines through the kit
        for _ in 0..(3 + r.next(8)) {
            let who = r.next(3);
            let text = f.text(conns[who]);
            let at = 1 + r.next(text.lines().count() + 1);
            let line = format!("{}-{made}", names[who]);
            made += 1;
            let notes = f.insert(conns[who], at, &format!("{line}\n"));
            got[who].extend(overwrites(&notes));
        }
        // an outside save removes some of them and replaces others; a stale buffer holds the
        // lines it kept
        let text = f.text(conns[0]);
        let mut saved = String::new();
        for l in text.lines() {
            let named = names.iter().find(|n| l.starts_with(&format!("{n}-")));
            if let (Some(n), 0) = (named, r.next(3)) {
                expect.entry(n).or_default().push(format!("{l}\n"));
                if r.next(2) == 0 {
                    saved.push_str(&format!("outside-{made}\n"));
                    made += 1;
                }
            } else {
                saved.push_str(l);
                saved.push('\n');
            }
        }
        f.save(conns[0], &saved);
    }
    for (k, n) in names.iter().enumerate() {
        let notes = f.s.take_notes(conns[k]).map_err(|e| e.to_string())?;
        got[k].extend(overwrites(&notes));
        let mut told: Vec<String> = Vec::new();
        for o in &got[k] {
            if o.more > 0 || o.not_shown > 0 {
                return Err(format!("seed {seed}: a small case was cut: {o:?}"));
            }
            told.extend(o.yours.lines().map(|l| format!("{l}\n")));
            for c in &o.also {
                told.extend(c.yours.lines().map(|l| format!("{l}\n")));
            }
        }
        told.sort();
        let mut want = expect.get(n).cloned().unwrap_or_default();
        want.sort();
        if told != want {
            return Err(format!(
                "seed {seed}: {n} was told {told:?}, removed {want:?}"
            ));
        }
    }
    Ok(())
}

#[test]
fn every_removal_within_the_rule_is_accounted_for() {
    for seed in 0..48 {
        check_case(seed).unwrap();
    }
}

/// Each planted fault is caught by the checks above.
#[test]
fn a_planted_fault_fails_the_checks() {
    let failed = differential::with_note_mutant(NoteMutant::DropAll, || {
        (0..8).find(|s| check_case(*s).is_err())
    });
    assert!(failed.is_some(), "dropping every note must fail the oracle");
    let failed = differential::with_note_mutant(NoteMutant::WrongAuthor, || {
        (0..8).find(|s| check_case(*s).is_err())
    });
    assert!(failed.is_some(), "the wrong author must fail the oracle");
    // the roll-up dropped: the exact count fails
    let n = OVERWRITE_PENDING_MAX + 6;
    let cells = differential::with_note_mutant(NoteMutant::DropRollup, || {
        let f = fx(V0);
        let agent = f.agent("agent");
        saves_removing_a_line_each(&f, agent, n);
        overwrites(&f.s.take_notes(agent).unwrap())
            .iter()
            .map(Overwrite::cells)
            .sum::<usize>()
    });
    assert_ne!(cells, n, "a dropped roll-up must fail the count");
}

#[test]
fn a_note_is_kind_overwritten_with_a_stable_name() {
    assert_eq!(NoteKind::Overwritten.as_str(), "overwritten");
}

// ----- the measurement -----

/// The cost under the session's lock on a whole-document delete. Two shapes: many lines from a
/// few changes, and many changes (a store lookup per run).
#[test]
#[ignore = "a measurement; run in release"]
fn removal_cost_on_a_whole_document_delete() {
    for (lines, edits) in [
        (10_000usize, 4usize),
        (100_000, 4),
        (5_000, 5_000),
        (20_000, 20_000),
    ] {
        let f = fx("seed\n");
        let conns: Vec<ConnId> = ["a", "b", "c", "d"].iter().map(|n| f.agent(n)).collect();
        let per = lines / edits;
        for e in 0..edits {
            let insert: String = (0..per)
                .map(|i| format!("line {e} {i} of the doc\n"))
                .collect();
            f.paste(conns[e % 4], &insert);
        }
        f.s.flush(conns[0]).unwrap();
        let bytes = std::fs::metadata(&f.file).unwrap().len();
        f.advance(10_000);
        std::fs::write(&f.file, "seed\n").unwrap();
        let t = std::time::Instant::now();
        f.s.file_changed().unwrap();
        let total = t.elapsed();
        let cost = differential::last_removal_cost();
        let told: usize = conns
            .iter()
            .map(|c| overwrites(&f.s.take_notes(*c).unwrap()).len())
            .sum();
        eprintln!(
            "[removal-cost] lines={lines} edits={edits} bytes={bytes} removal_us={} save_us={} notes={told}",
            cost.as_micros(),
            total.as_micros()
        );
        let job = differential::runrec_job();
        eprintln!(
            "RUNREC {{\"job\":{job},\"run\":\"removal_cost_{lines}_{edits}\",\"tool\":\"outside_removal_notes\",\"status\":\"COMPLETE\",\"lines\":{lines},\"edits\":{edits},\"bytes\":{bytes},\"removal_us\":{},\"save_us\":{},\"notes\":{told}}}",
            cost.as_micros(),
            total.as_micros()
        );
    }
}
