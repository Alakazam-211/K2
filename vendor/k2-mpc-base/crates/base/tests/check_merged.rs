//! A merge the pack refuses (`Pack::check_merged`): some formats' lines depend on each other,
//! so two valid texts can merge, line by line, into one that isn't valid. The kit asks the
//! pack, and on a refusal an outside save is taken whole (the live text it replaces stays in
//! the version before it, and the removal note names that version), and `write --base` is a
//! conflict with nothing applied. A text that was already refused before the merge is the
//! writer's own, and merges as before.
//!
//! The test pack's format: every line is a number, and the numbers strictly increase. Two
//! changes to neighbouring lines, each valid, can merge out of order.
//!
//! Each row is a function giving `Err` (naming the row) when the guard doesn't hold, so the
//! planted fault (the kit's call skipped) is shown to turn each one red by name.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::{self, CheckMergedMutant};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, DiskCheck, Host, NoteKind, Notes, Overwrite, Pack,
    PackContent, PackError, Participant, ParticipantKind, Rights, Session, SessionManager, Stay,
    TextFilePack, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::Schema;

const ROOT: &str = TextFilePack::ROOT;

/// A text file whose lines are numbers in strictly increasing order.
struct Sorted(TextFilePack);

fn sorted(text: &str) -> bool {
    let mut last: Option<i64> = None;
    for l in text.lines() {
        let Ok(n) = l.trim().parse::<i64>() else {
            return false;
        };
        if last.is_some_and(|p| p >= n) {
            return false;
        }
        last = Some(n);
    }
    true
}

impl Pack for Sorted {
    fn schema(&self) -> Schema {
        self.0.schema()
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        self.0.decode(bytes)
    }
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        self.0.encode(content, codec)
    }
    fn normalize_input<'a>(&self, root: &str, text: &'a str) -> Cow<'a, str> {
        self.0.normalize_input(root, text)
    }
    fn tokens(&self) -> k2_mpc_base::text::lines::Tokens {
        self.0.tokens()
    }
    fn check_merged(&self, root: &str, text: &str) -> Result<(), Declined> {
        assert_eq!(root, ROOT, "the kit asks about the doc's text root");
        if sorted(text) {
            Ok(())
        } else {
            Err(Declined {
                code: "unsorted",
                message: "the lines are not in increasing order".into(),
            })
        }
    }
}

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
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("k2mb-checkmerged-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let file = dir.join("ws/numbers.txt");
    std::fs::write(&file, initial).unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
        events: Mutex::new(Vec::new()),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: "numbers.txt".into(),
    };
    let pack = Arc::new(Sorted(TextFilePack::new("numbers", 4 << 20)));
    let s = mgr.open_file(pack, &key, &file, &dir.join("ws")).unwrap();
    Fx {
        dir,
        file,
        host,
        mgr,
        s,
    }
}

impl Fx {
    fn agent(&self, pid: &str) -> ConnId {
        self.s
            .join(
                Participant::new(pid, pid, "#1", ParticipantKind::Agent, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn text(&self, c: ConnId) -> String {
        self.s.read(c).unwrap().text_by_root[0].1.clone()
    }
    fn file(&self) -> String {
        std::fs::read_to_string(&self.file).unwrap()
    }
    /// `c`'s next read: what its notes say.
    fn cat(&self, c: ConnId) -> Notes {
        self.s
            .run_tool(c, ROOT, &["cat".to_owned()], b"")
            .unwrap()
            .notes
    }
    fn read_version(&self, c: ConnId, v: u64) -> String {
        self.s.show(c, v).unwrap()[0].1.clone()
    }
    /// `c` writes `text` from the version it reads now: applied.
    fn write(&self, c: ConnId, text: &str) {
        let v = self.s.read(c).unwrap().version;
        let out = self.s.write_base(c, ROOT, v, text).unwrap();
        assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
    }
    /// An editor saves `text` over the file (the kit hasn't written the live edits back).
    fn save(&self, text: &str) -> DiskCheck {
        self.advance(10_000);
        std::fs::write(&self.file, text).unwrap();
        self.s.file_changed().unwrap()
    }
}

fn overwrites(n: &Notes) -> Vec<Overwrite> {
    n.notes
        .iter()
        .filter(|x| x.kind == NoteKind::Overwritten)
        .filter_map(|x| x.overwrite.clone())
        .collect()
}

/// `Err(row: what)` unless `ok`.
fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {}", what()))
    }
}

const BASE: &str = "1\n5\n9\n";

/// The agent's change is live, not yet written back, when an editor saves a change to the same
/// line. Kept both, the lines would be `1 6 4 9`: out of order. The save is taken whole: the
/// doc and the file hold the person's text, and the agent's line is in the version the note
/// names, and in the note itself.
fn row_outside_save_taken_whole() -> Result<(), String> {
    const ROW: &str = "row_outside_save_taken_whole";
    let f = fx(BASE);
    let agent = f.agent("agent");
    f.write(agent, "1\n6\n9\n");
    let check = f.save("1\n4\n9\n");
    need(matches!(check, DiskCheck::Ingested { .. }), ROW, || {
        format!("the save is taken in: {check:?}")
    })?;
    let now = f.text(agent);
    need(now == "1\n4\n9\n", ROW, || {
        format!("the doc is the save: {now:?}")
    })?;
    f.s.flush(agent).unwrap();
    let file = f.file();
    need(file == "1\n4\n9\n", ROW, || {
        format!("the file is the save: {file:?}")
    })?;
    let notes = f.cat(agent);
    let o = overwrites(&notes);
    need(o.len() == 1, ROW, || format!("one removal note: {notes:?}"))?;
    need(o[0].yours.contains('6'), ROW, || {
        format!("the note carries the agent's line: {:?}", o[0].yours)
    })?;
    let v = o[0]
        .version
        .ok_or_else(|| format!("{ROW}: the note names a version"))?;
    let held = f.read_version(agent, v);
    need(held == "1\n6\n9\n", ROW, || {
        format!("version {v} holds the agent's text: {held:?}")
    })
}

/// Two agents write from one version, each changing a neighbouring line. The strict merge is
/// clean, but its lines `4 3 9` are out of order: the second write is one conflict over both
/// lines, nothing applied. It re-reads and writes again, and that lands.
fn row_write_base_refused_is_a_conflict() -> Result<(), String> {
    const ROW: &str = "row_write_base_refused_is_a_conflict";
    let f = fx(BASE);
    let (a, b) = (f.agent("agent-a"), f.agent("agent-b"));
    let v = f.s.read(a).unwrap().version;
    let first = f.s.write_base(a, ROOT, v, "1\n3\n9\n").unwrap();
    need(matches!(first, WriteOutcome::Applied(_)), ROW, || {
        format!("the first write lands: {first:?}")
    })?;
    match f.s.write_base(b, ROOT, v, "4\n5\n9\n").unwrap() {
        WriteOutcome::Conflicts(c) => {
            need(c.len() == 1, ROW, || format!("one region: {c:?}"))?;
            need(c[0].lines == (1, 2), ROW, || {
                format!("over both changed lines: {:?}", c[0].lines)
            })?;
            need(
                c[0].ours == "4\n5\n" && c[0].theirs == "1\n3\n",
                ROW,
                || format!("each side's lines: {c:?}"),
            )?;
        }
        other => return Err(format!("{ROW}: the refused merge applied: {other:?}")),
    }
    let now = f.text(a);
    need(now == "1\n3\n9\n", ROW, || {
        format!("nothing applied: {now:?}")
    })?;
    f.write(b, "2\n3\n9\n");
    let now = f.text(a);
    need(now == "2\n3\n9\n", ROW, || {
        format!("the retry lands: {now:?}")
    })
}

#[test]
fn an_outside_save_the_pack_refuses_is_taken_whole() {
    row_outside_save_taken_whole().unwrap();
}

#[test]
fn a_write_base_the_pack_refuses_is_a_conflict_and_applies_nothing() {
    row_write_base_refused_is_a_conflict().unwrap();
}

/// Merges the pack accepts are as before, on both paths.
#[test]
fn merges_the_pack_accepts_are_as_before() {
    // outside save: the agent's line and the person's, different lines, both kept
    let f = fx(BASE);
    let agent = f.agent("agent");
    f.write(agent, "1\n5\n8\n");
    assert!(matches!(f.save("2\n5\n9\n"), DiskCheck::Ingested { .. }));
    assert_eq!(f.text(agent), "2\n5\n8\n");
    assert!(overwrites(&f.cat(agent)).is_empty(), "nothing removed");
    // write --base: two lines apart, in order
    let f = fx(BASE);
    let (a, b) = (f.agent("agent-a"), f.agent("agent-b"));
    let v = f.s.read(a).unwrap().version;
    f.write(a, "1\n5\n8\n");
    let out = f.s.write_base(b, ROOT, v, "2\n5\n9\n").unwrap();
    assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
    assert_eq!(f.text(a), "2\n5\n8\n");
}

/// A text the pack already refused before the merge is the writer's own: the merge goes ahead
/// as before (kept both, for an outside save), not the save taken whole.
#[test]
fn a_writers_own_refused_text_merges_as_before() {
    let f = fx(BASE);
    let agent = f.agent("agent");
    f.write(agent, "1\n6\n9\n");
    // the person's own save is out of order
    assert!(matches!(f.save("1\n4\n3\n9\n"), DiskCheck::Ingested { .. }));
    let now = f.text(agent);
    assert!(now.contains("6\n"), "kept both, as before: {now:?}");
    // write --base: the writer's own text is out of order; it applies as before
    let f = fx(BASE);
    let a = f.agent("agent-a");
    let v = f.s.read(a).unwrap().version;
    let out = f.s.write_base(a, ROOT, v, "9\n5\n1\n").unwrap();
    assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
}

/// The planted fault: the kit's call skipped. Each row above goes red, by its name.
#[test]
fn the_planted_skipped_check_is_caught_by_name() {
    differential::with_check_merged_mutant(CheckMergedMutant::Skipped, || {
        for (name, row) in [
            (
                "row_outside_save_taken_whole",
                row_outside_save_taken_whole as fn() -> Result<(), String>,
            ),
            (
                "row_write_base_refused_is_a_conflict",
                row_write_base_refused_is_a_conflict,
            ),
        ] {
            let e = row().expect_err(&format!("planted, {name} must fail"));
            assert!(e.starts_with(&format!("{name}: ")), "{e}");
        }
    });
}
