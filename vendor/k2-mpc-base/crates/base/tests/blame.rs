//! Blame (K-HIST-11, K-HIST-18, P22) through sessions: every kind of author (the import, a
//! person, agents, an assistant acting for someone, an outside edit), the most-recent rule
//! inside one line, `-L` ranges, undo / redo / revert / restore attributed to whoever ran
//! them (undo spec U18a), purged changes marked `erased` and erased participants shown by
//! pseudonym, the history right, tier 0's pointer blame, and a property test against a
//! model of who typed every character. Public API only.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    Action, AnyFile, ApprovalState, AuditEvent, Blame, BlameLine, ChangeKind, Config, ConnId, Host,
    LineRange, Need, Participant, ParticipantKind, PurgeRequest, PurgeScope, Rights, Session,
    SessionError, SessionManager, Stay, TextFilePack, UndoMode, UndoOutcome, UndoRequest,
    EXTERNAL_PID, IMPORT_PID,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::tools::session_exit;
use k2_mpc_base::Edit;
use proptest::prelude::*;

const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;
const T0: u64 = 1_793_111_400_000;

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

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-blame-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(T0),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    let f = Fx { dir, ws, host, mgr };
    f.grant("own", Rights::full().with_owner(true));
    for p in ["alice", "bob", "carol", "aide", "bot", "obs"] {
        f.grant(p, Rights::full());
    }
    f
}

impl Fx {
    fn grant(&self, pid: &str, r: Rights) {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn set_now(&self, ms: u64) {
        self.host.now.store(ms, Ordering::SeqCst);
    }
    fn now(&self) -> u64 {
        self.host.now.load(Ordering::SeqCst)
    }
    /// A text doc synced with file `name`, imported from `text`.
    fn open(&self, name: &str, text: &str) -> Arc<Session> {
        let p = self.ws.join(name);
        fs::write(&p, text).unwrap();
        let key = DocKey {
            space: "ws".into(),
            path: name.into(),
        };
        self.mgr
            .open_file(Arc::new(MD), &key, &p, &self.ws)
            .unwrap()
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
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

fn join(s: &Session, p: Participant) -> ConnId {
    s.join(p, Stay::Held).unwrap().conn
}

/// Replace the first `old` with `new` through `c`; returns the change's log position.
fn replace(s: &Session, c: ConnId, old: &str, new: &str) -> u64 {
    s.edit_with(c, ROOT, |t| match t.find(old) {
        Some(_) => Ok(t.replacen(old, new, 1)),
        None => Err(format!("{old:?} not in {t:?}")),
    })
    .unwrap()
    .unwrap_or_else(|| panic!("replace {old:?} changed nothing"))
    .version
}

/// One hunk at exact byte positions.
fn hunk(s: &Session, c: ConnId, start: usize, end: usize, insert: &str) -> u64 {
    s.edit(
        c,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start,
                end,
                insert: insert.into(),
            }],
        }],
        None,
    )
    .unwrap()
    .version
}

fn blame(s: &Session, c: ConnId) -> Blame {
    s.blame(c, ROOT, None).unwrap()
}

fn range(first: usize, last: usize) -> Option<LineRange> {
    Some(LineRange {
        root: ROOT.into(),
        first,
        last,
    })
}

/// `(line, author, change)` per line.
fn who(b: &Blame) -> Vec<(usize, String, Option<u64>)> {
    b.lines
        .iter()
        .map(|l| (l.line, l.author.clone(), l.change))
        .collect()
}

fn line(b: &Blame, n: usize) -> &BlameLine {
    b.lines.iter().find(|l| l.line == n).expect("line")
}

// ---------- every kind of author ----------

/// IT `blame::agent_person_external` (K-HIST-11, K-ID-8, K-ID-10, K-HIST-18): the import, a
/// person, two agents, an assistant acting for the person (pending), and an outside edit,
/// each on its own line, with change, kind, surface, time, on-behalf and approval.
#[test]
fn blame_agent_person_external() {
    let f = fx("kinds");
    let s = f.open(
        "k.md",
        "# Title\nline two\nline three\nline four\nline five\n",
    );
    let alice = join(&s, person("alice"));
    let bot = join(&s, agent("bot"));
    let bob = join(&s, agent("bob"));
    let aide = join(&s, agent("aide").on_behalf_of("alice"));
    let obs = join(&s, person("obs"));
    let t_import = f.now();
    f.advance(1_000);
    let c_alice = replace(&s, alice, "line two", "line 2 by alice");
    f.advance(1_000);
    let c_bot = replace(&s, bot, "line three", "line 3 by bot");
    f.advance(1_000);
    let c_bob = replace(&s, bob, "line four", "line 4 by bob");
    f.advance(1_000);
    let c_aide = replace(&s, aide, "line five", "line 5 by aide");
    f.advance(1_000);
    // another program appends a line (after the kit wrote the file back)
    s.flush(alice).unwrap();
    let file = f.ws.join("k.md");
    let mut t = fs::read_to_string(&file).unwrap();
    t.push_str("line six, from an editor\n");
    fs::write(&file, &t).unwrap();
    s.file_changed().unwrap();
    let t_ext = f.now();

    let b = blame(&s, obs);
    assert_eq!(b.root, ROOT);
    assert_eq!(b.version, s.status().unwrap().version);
    assert_eq!(b.lines.len(), 6, "no empty line after the final newline");
    let texts: Vec<&str> = b.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "# Title",
            "line 2 by alice",
            "line 3 by bot",
            "line 4 by bob",
            "line 5 by aide",
            "line six, from an editor"
        ]
    );
    let authors: Vec<&str> = b.lines.iter().map(|l| l.author.as_str()).collect();
    assert_eq!(
        authors,
        [IMPORT_PID, "alice", "bot", "bob", "aide", EXTERNAL_PID]
    );
    let l1 = line(&b, 1);
    assert_eq!(
        (l1.change, l1.kind, l1.via.as_deref(), l1.at_ms),
        (
            Some(1),
            Some(ChangeKind::Import),
            Some("disk"),
            Some(t_import)
        )
    );
    let l2 = line(&b, 2);
    assert_eq!(
        (l2.change, l2.kind, l2.via.as_deref()),
        (Some(c_alice), Some(ChangeKind::Edit), Some("tab"))
    );
    assert_eq!(line(&b, 3).change, Some(c_bot));
    assert_eq!(line(&b, 3).via.as_deref(), Some("cli"));
    assert_eq!(line(&b, 4).change, Some(c_bob));
    let l5 = line(&b, 5);
    assert_eq!(l5.change, Some(c_aide));
    assert_eq!(l5.on_behalf_of.as_deref(), Some("alice"));
    assert_eq!(l5.approval, Some(ApprovalState::Pending));
    let l6 = line(&b, 6);
    assert_eq!(
        (l6.kind, l6.via.as_deref(), l6.at_ms),
        (Some(ChangeKind::External), Some("disk"), Some(t_ext))
    );
    assert!(b.lines.iter().all(|l| !l.erased));
    // the approval state follows the approval
    s.set_approval(alice, c_aide, ApprovalState::Approved)
        .unwrap();
    assert_eq!(
        line(&blame(&s, obs), 5).approval,
        Some(ApprovalState::Approved)
    );

    // the listing a host prints: labels from its lookup, the pid otherwise
    let names = |pid: &str| (pid == "alice").then(|| "Alice A.".to_owned());
    let out = blame(&s, obs).render(names);
    let rows: Vec<&str> = out.lines().collect();
    assert_eq!(rows.len(), 6);
    assert!(rows[0].starts_with("1 (kit:import "), "{out}");
    assert!(rows[0].ends_with(" 1) # Title"), "{out}");
    assert!(rows[0].contains(" 2026-10-27 "), "{out}");
    assert!(rows[1].contains("(Alice A. "), "{out}");
    assert!(rows[4].contains("aide for Alice A. "), "{out}");
    assert!(!rows[4].contains("(pending)"), "approved now: {out}");
    // columns line up
    let paren: Vec<usize> = rows.iter().map(|r| r.find(") ").unwrap()).collect();
    assert!(paren.windows(2).all(|w| w[0] == w[1]), "{out}");
    // and the porcelain form
    let p = blame(&s, obs).porcelain();
    assert!(
        p.starts_with("1 1 1 1\nauthor kit:import\nauthor-time "),
        "{p}"
    );
    assert!(p.contains(&format!("{c_aide} 5 5 1\nauthor aide\n")), "{p}");
    assert!(
        p.contains("on-behalf-of alice\napproval approved\n\tline 5 by aide\n"),
        "{p}"
    );
    assert!(p.contains("kind external\nvia disk\n\tline six"), "{p}");
}

// ---------- the most-recent rule ----------

/// Interleaved edits inside one line: the line goes to the character with the greatest
/// change id (apply order), whatever the host clock says: a later change stamped with an
/// earlier time still wins, and so does the later of two in one millisecond; a `\n` typed
/// into a line attributes the first half to its typist.
#[test]
fn blame_most_recent_author_within_a_line() {
    let f = fx("recent");
    let s = f.open("r.md", "aaaa bbbb cccc\nzzzz\n");
    let alice = join(&s, person("alice"));
    let bob = join(&s, agent("bob"));
    let carol = join(&s, person("carol"));
    f.advance(10);
    replace(&s, bob, "bbbb", "BBBB");
    assert_eq!(line(&blame(&s, alice), 1).author, "bob");
    f.advance(10);
    let a = replace(&s, alice, "aaaa", "AAAA");
    let b = blame(&s, alice);
    assert_eq!(
        who(&b)[0],
        (1, "alice".to_owned(), Some(a)),
        "the most recent wins"
    );
    assert_eq!(who(&b)[1].1, IMPORT_PID, "line 2 untouched");
    // the host clock steps back: carol's later change carries an earlier time, and wins
    f.set_now(T0 + 5);
    let late = replace(&s, carol, "cccc", "CCCC");
    let l = line(&blame(&s, alice), 1).clone();
    assert_eq!(
        (l.author.as_str(), l.change, l.at_ms),
        ("carol", Some(late), Some(T0 + 5))
    );
    assert!(late > a, "a later change id");
    // same millisecond: the later change id wins
    f.set_now(T0 + 100);
    replace(&s, bob, "BBBB", "bbbb");
    let c = replace(&s, carol, "CCCC", "cccc");
    assert_eq!(
        (
            line(&blame(&s, alice), 1).author.as_str(),
            line(&blame(&s, alice), 1).change
        ),
        ("carol", Some(c))
    );
    // bob splits line 2 by typing a newline: its first half ends in his `\n`
    f.advance(10);
    let t = s.read(alice).unwrap().text_by_root.remove(0).1;
    let at = t.find("zz").unwrap() + 2;
    let split = hunk(&s, bob, at, at, "\n");
    let b = blame(&s, alice);
    assert_eq!(
        who(&b)[1..],
        [
            (2, "bob".to_owned(), Some(split)),
            (3, IMPORT_PID.to_owned(), Some(1)),
        ]
    );
    assert_eq!(
        (line(&b, 2).text.as_str(), line(&b, 3).text.as_str()),
        ("zz", "zz")
    );
}

// ---------- -L ----------

#[test]
fn blame_line_ranges() {
    let f = fx("ranges");
    let s = f.open("l.md", "one\ntwo\nthree\nfour\n");
    let alice = join(&s, person("alice"));
    let two = replace(&s, alice, "two", "TWO");
    let all = blame(&s, alice);
    let b = s.blame(alice, ROOT, range(2, 3)).unwrap();
    assert_eq!(b.lines, all.lines[1..3].to_vec());
    assert_eq!(who(&b)[0], (2, "alice".to_owned(), Some(two)));
    // past the end is clamped; a single line
    assert_eq!(s.blame(alice, ROOT, range(3, 99)).unwrap().lines.len(), 2);
    assert_eq!(
        s.blame(alice, ROOT, range(4, 4)).unwrap().lines[0].text,
        "four"
    );
    // refused: line 0, a reversed range, a first line past the end (the empty line after
    // the final newline is no line), another root's range
    for (a, z) in [(0, 2), (3, 2), (5, 5), (9, 12)] {
        assert!(
            matches!(
                s.blame(alice, ROOT, range(a, z)),
                Err(SessionError::BadPosition(_))
            ),
            "{a},{z}"
        );
    }
    let other = Some(LineRange {
        root: "notes".into(),
        first: 1,
        last: 1,
    });
    assert!(matches!(
        s.blame(alice, ROOT, other),
        Err(SessionError::BadPosition(_))
    ));
    assert!(s.blame(alice, "nope", None).is_err());
    // a last line without a newline counts; an empty doc has no lines
    replace(&s, alice, "four\n", "four");
    let b = blame(&s, alice);
    assert_eq!(b.lines.len(), 4);
    assert_eq!(b.lines[3].text, "four");
    s.replace_text(alice, ROOT, "").unwrap();
    assert!(blame(&s, alice).lines.is_empty());
    assert!(s.blame(alice, ROOT, range(1, 1)).is_err());
}

// ---------- undo, redo, revert, restore ----------

/// Undo spec U18a: text undo or redo brings back is new content by whoever ran it, at that
/// time; the same for revert and restore (each a change by the caller).
#[test]
fn blame_restored_text_is_the_restorers() {
    let f = fx("undo");
    let s = f.open("u.md", "keep\n");
    let alice = join(&s, person("alice"));
    let bob = join(&s, agent("bob"));
    let aide = join(&s, agent("aide").on_behalf_of("alice"));
    let obs = join(&s, person("obs"));
    f.advance(10);
    replace(&s, alice, "keep\n", "keep\nalice's line\n");
    f.advance(10);
    // bob deletes alice's line, then undoes his delete: the line is back, as bob's
    replace(&s, bob, "alice's line\n", "");
    f.advance(10);
    let out = s
        .undo_with(bob, &UndoRequest::steps(1).mode(UndoMode::All))
        .unwrap();
    let UndoOutcome::Applied { result, .. } = out else {
        panic!("{out:?}")
    };
    let l = line(&blame(&s, obs), 2).clone();
    assert_eq!(l.text, "alice's line");
    assert_eq!(
        (l.author.as_str(), l.kind, l.change, l.at_ms),
        (
            "bob",
            Some(ChangeKind::Undo),
            Some(result.version),
            Some(f.now())
        )
    );
    // alice's own edit, undone and redone: the redo is hers, a new change
    f.advance(10);
    replace(&s, alice, "keep", "KEEP");
    let UndoOutcome::Applied { result, .. } = s.undo(alice, 1).unwrap() else {
        panic!()
    };
    // the undo re-inserts the import's "keep": new content, alice's (U18a)
    let l = line(&blame(&s, obs), 1).clone();
    assert_eq!(
        (l.text.as_str(), l.author.as_str(), l.kind, l.change),
        (
            "keep",
            "alice",
            Some(ChangeKind::Undo),
            Some(result.version)
        )
    );
    f.advance(10);
    let UndoOutcome::Applied { result, .. } = s.redo(alice, 1).unwrap() else {
        panic!()
    };
    let l = line(&blame(&s, obs), 1).clone();
    assert_eq!(
        (l.author.as_str(), l.kind, l.change),
        ("alice", Some(ChangeKind::Redo), Some(result.version))
    );
    // an assistant deletes a line for alice; alice reverts it: the line is alice's again
    f.advance(10);
    let gone = replace(&s, aide, "alice's line\n", "");
    f.advance(10);
    let UndoOutcome::Applied { result, .. } = s.revert(alice, gone, false).unwrap() else {
        panic!()
    };
    let l = line(&blame(&s, obs), 2).clone();
    assert_eq!(
        (
            l.author.as_str(),
            l.kind,
            l.change,
            l.on_behalf_of.as_deref()
        ),
        (
            "alice",
            Some(ChangeKind::Revert),
            Some(result.version),
            None
        )
    );
    // restoring the import's version: the restorer's
    let v1 = s.versions(obs).unwrap()[0].id;
    f.advance(10);
    let r = s.restore(bob, v1).unwrap();
    let b = blame(&s, obs);
    assert_eq!(b.lines.len(), 1);
    assert_eq!(
        (
            b.lines[0].author.as_str(),
            b.lines[0].kind,
            b.lines[0].change
        ),
        ("bob", Some(ChangeKind::Restore), Some(r.version))
    );
}

// ---------- erasure ----------

/// A purged change's line is `erased` (its record's content is gone), with its id, time
/// and author kept as amendment 7 keeps them; participant erasure shows the pseudonym; a
/// purge that removes the text leaves no line.
#[test]
fn blame_after_purge() {
    let f = fx("purge");
    let s = f.open("p.md", "intro\n");
    let own = join(&s, person("own"));
    let alice = join(&s, person("alice"));
    let bob = join(&s, agent("bob"));
    f.advance(10);
    let pasted = replace(&s, alice, "intro\n", "intro\nkey=0123456789abcdef0123\n");
    f.advance(10);
    let bobs = replace(&s, bob, "intro\n", "intro by bob\n");
    s.purge(
        own,
        PurgeRequest::new(PurgeScope::Changes {
            from: pasted,
            to: pasted,
        }),
    )
    .unwrap();
    let b = blame(&s, own);
    let l = line(&b, 2);
    assert!(l.erased, "{l:?}");
    assert_eq!(
        (l.author.as_str(), l.change, l.kind),
        ("alice", Some(pasted), Some(ChangeKind::Edit))
    );
    assert!(!line(&b, 1).erased);
    // participant erasure: bob's line shows his pseudonym, erased
    s.purge(
        own,
        PurgeRequest::new(PurgeScope::Participant {
            pid: "bob".into(),
            pseudonym: "participant-7".into(),
        }),
    )
    .unwrap();
    let b = blame(&s, own);
    let l = line(&b, 1);
    assert_eq!(
        (l.author.as_str(), l.change, l.erased),
        ("participant-7", Some(bobs), true)
    );
    assert!(b.lines.iter().all(|l| l.author != "bob"));
    assert!(!b.porcelain().contains("author bob"));
    assert!(b.render(|_| None).contains("[erased]"));
    // purging the text with and_remove: the line is gone from the doc and from blame
    s.purge(
        own,
        PurgeRequest::new(PurgeScope::Text("0123456789abcdef0123".into())).and_remove(),
    )
    .unwrap();
    let b = blame(&s, own);
    assert!(
        b.lines.iter().all(|l| !l.text.contains("0123456789")),
        "{b:?}"
    );
    assert_eq!(b.lines.len(), 2);
}

// ---------- rights ----------

/// K-PERM-9: blame needs history, not just read or edit; refused and audited.
#[test]
fn blame_needs_history() {
    let f = fx("rights");
    let s = f.open("h.md", "x\n");
    f.grant("ed", Rights::editor());
    f.grant("reader", Rights::read_only().with_history(true));
    let ed = join(&s, person("ed"));
    let reader = join(&s, person("reader"));
    let e = s.blame(ed, ROOT, None).unwrap_err();
    assert_eq!(
        e,
        SessionError::Forbidden {
            need: Need::History
        }
    );
    assert_eq!(
        session_exit(&e),
        session_exit(&SessionError::Forbidden {
            need: Need::History
        })
    );
    assert!(f.events().iter().any(|ev| matches!(
        ev,
        AuditEvent::Refused { pid, action: Action::Blame, need: Need::History, .. } if pid == "ed"
    )));
    // still present: a missing history right doesn't remove anyone
    assert!(s.roster(ed).is_ok());
    // read + history is enough
    assert_eq!(blame(&s, reader).lines.len(), 1);
}

// ---------- tier 0 ----------

/// Tier 0 (whole-blob grain): the current version's setter and the pointer's history.
#[test]
fn blob_blame_is_the_pointer_history() {
    let f = fx("blob");
    let p = f.ws.join("a.bin");
    fs::write(&p, b"\x00one").unwrap();
    let key = DocKey {
        space: "ws".into(),
        path: "a.bin".into(),
    };
    let s = f
        .mgr
        .open_blob_file(Arc::new(AnyFile), &key, &p, &f.ws)
        .unwrap();
    let alice = s.join(person("alice"), Stay::Held).unwrap().conn;
    let bob = s.join(agent("bob"), Stay::Held).unwrap().conn;
    let write = |c: ConnId, bytes: &[u8]| {
        f.advance(10);
        let base = s.status().unwrap().version;
        s.write_bytes(c, bytes, base).unwrap()
    };
    write(alice, b"\x00two");
    let w = write(bob, b"\x00three");
    write(alice, b"\x00four");
    let last = write(bob, b"\x00five");
    let b = s.blame(alice).unwrap();
    assert_eq!(b.version, last.version);
    assert_eq!(b.current.author, "bob");
    assert_eq!(b.current.at_ms, f.now());
    assert_eq!(b.versions, 5);
    let authors: Vec<(&str, usize)> = b
        .authors
        .iter()
        .map(|a| (a.pid.as_str(), a.versions))
        .collect();
    assert_eq!(authors, [("bob", 2), ("alice", 2), (IMPORT_PID, 1)]);
    assert_eq!(b.authors[0].last_ms, f.now());
    assert!(w.version < last.version);
    // history right
    f.grant("ed", Rights::editor());
    let ed = s.join(person("ed"), Stay::Held).unwrap().conn;
    assert_eq!(
        s.blame(ed).unwrap_err(),
        SessionError::Forbidden {
            need: Need::History
        }
    );
}

// ---------- the property ----------

/// One participant's edit: replace `del` characters at `at` (both reduced modulo the text)
/// with `ins`, after the clock moves `dt` ms (0: the same millisecond; negative: skew).
#[derive(Debug, Clone)]
struct Op {
    who: usize,
    at: usize,
    del: usize,
    ins: String,
    dt: i64,
}

fn op() -> impl Strategy<Value = Op> {
    (0..3usize, 0..400usize, 0..4usize, "[ab\n]{0,5}", -3..3i64).prop_map(
        |(who, at, del, ins, dt)| Op {
            who,
            at,
            del,
            ins,
            dt,
        },
    )
}

const WHO: [&str; 3] = ["alice", "bob", "bot"];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Against a model of who typed every character (and in which change, when): each
    /// line's author is one of the participants who wrote a character on it, and exactly
    /// the one the rule picks; its change and time match; its text is the line's.
    #[test]
    fn blame_author_wrote_on_the_line(ops in prop::collection::vec(op(), 1..30)) {
        let f = fx("prop");
        let s = f.open("m.md", "seed line\nsecond\n");
        let conns: Vec<ConnId> = WHO
            .iter()
            .enumerate()
            .map(|(i, p)| join(&s, if i == 1 { agent(p) } else { person(p) }))
            .collect();
        // (byte, writer, change, time): writer 3 is the import
        let mut model: Vec<(u8, usize, u64, u64)> =
            b"seed line\nsecond\n".iter().map(|&c| (c, 3, 1, T0)).collect();
        for o in &ops {
            // the host clock may step back (skew): blame must not care
            f.set_now(f.now().saturating_add_signed(o.dt));
            let at = o.at % (model.len() + 1);
            let end = (at + o.del).min(model.len());
            if at == end && o.ins.is_empty() {
                continue;
            }
            let seq = hunk(&s, conns[o.who], at, end, &o.ins);
            let now = f.now();
            model.splice(at..end, o.ins.bytes().map(|c| (c, o.who, seq, now)));
        }
        let text: Vec<u8> = model.iter().map(|m| m.0).collect();
        let live = s.read(conns[0]).unwrap().text_by_root.remove(0).1;
        prop_assert_eq!(live.as_bytes(), &text[..]);
        let b = blame(&s, conns[0]);
        let mut lines = Vec::new();
        let mut cur: Vec<(u8, usize, u64, u64)> = Vec::new();
        for m in &model {
            cur.push(*m);
            if m.0 == b'\n' {
                lines.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        prop_assert_eq!(b.lines.len(), lines.len());
        for (l, chars) in b.lines.iter().zip(&lines) {
            let name = |w: usize| if w == 3 { IMPORT_PID } else { WHO[w] };
            let wrote: Vec<&str> = chars.iter().map(|c| name(c.1)).collect();
            prop_assert!(wrote.contains(&l.author.as_str()), "{:?} not in {:?}", l, wrote);
            // the rule: the greatest change id (never the time), the first such character
            let mut best = chars[0];
            for c in chars {
                if c.2 > best.2 {
                    best = *c;
                }
            }
            prop_assert_eq!(l.author.as_str(), name(best.1));
            prop_assert_eq!(l.change, Some(best.2));
            prop_assert_eq!(l.at_ms, Some(best.3));
            let t: Vec<u8> = chars.iter().map(|c| c.0).filter(|&c| c != b'\n').collect();
            prop_assert_eq!(l.text.as_bytes(), &t[..]);
        }
    }
}
