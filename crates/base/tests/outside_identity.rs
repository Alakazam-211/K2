//! An outside save's change at element identity, taken from the merge's own hunks (v0.9.5
//! perf), against the full computation from the engine's element IDs (v0.9.3's path): the
//! same, over random docs of repeated lines (the case that made identity necessary: a diff
//! of the texts names other elements there), with agent edits not yet written back,
//! write-backs, and outside saves made on the file. `Config::edit_check_every = 1` makes every
//! outside save compute both and compare; any difference is an
//! `AuditEvent::IdentityMismatch`, and the run fails on one. Public API only.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::{
    AuditEvent, Config, DiskCheck, Host, Participant, ParticipantKind, Rights, SessionManager,
    Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use proptest::prelude::*;

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

struct H {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
}

/// A line edit: `(kind, at, letter)` on a text of lines (kind 0 insert, 1 delete, 2 replace,
/// 3 duplicate the line at `at` next to it).
type Op = (u8, usize, u8);

fn apply(text: &str, (kind, at, letter): Op) -> String {
    let mut lines: Vec<String> = text.lines().map(|l| format!("{l}\n")).collect();
    let line = format!("{}\n", ["A", "B", "X"][letter as usize % 3]);
    let n = lines.len();
    match kind % 4 {
        0 => lines.insert(at % (n + 1), line),
        1 if n > 1 => {
            lines.remove(at % n);
        }
        2 if n > 0 => lines[at % n] = line,
        3 if n > 0 => {
            let k = at % n;
            let dup = lines[k].clone();
            lines.insert(k + 1, dup);
        }
        _ => lines.push(line),
    }
    lines.concat()
}

/// One step: an agent's edit (not written back), an outside save of the file, or a write-back.
#[derive(Debug, Clone)]
enum Step {
    Agent(usize, Op),
    Outside(Op),
    Flush,
}

fn step() -> impl Strategy<Value = Step> {
    let op = (0u8..4, 0usize..16, 0u8..3);
    prop_oneof![
        3 => (0usize..2, op.clone()).prop_map(|(a, o)| Step::Agent(a, o)),
        4 => op.prop_map(Step::Outside),
        1 => Just(Step::Flush),
    ]
}

fn run(seed_lines: &[u8], steps: &[Step]) -> (usize, Vec<String>) {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-outside-identity-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let file = dir.join("ws").join("a.md");
    let seed: String = seed_lines
        .iter()
        .map(|l| format!("{}\n", ["A", "B", "X"][*l as usize % 3]))
        .collect();
    std::fs::write(&file, &seed).unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000),
        events: Mutex::new(Vec::new()),
    });
    let mut cfg = Config::default();
    cfg.edit_check_every = 1;
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), cfg);
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let s = mgr
        .open_file(Arc::new(MD), &key, &file, &dir.join("ws"))
        .unwrap();
    let agents: Vec<_> = (0..2)
        .map(|a| {
            s.join(
                Participant::new(format!("a{a}"), "A", "#1", ParticipantKind::Agent, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn
        })
        .collect();
    let mut ingested = 0;
    for st in steps {
        host.now.fetch_add(100, Ordering::SeqCst);
        match st {
            Step::Agent(a, op) => {
                let c = agents[*a];
                let t = s.read(c).unwrap().text_by_root.remove(0).1;
                let _ = s.replace_text(c, ROOT, &apply(&t, *op));
            }
            Step::Outside(op) => {
                let t = std::fs::read_to_string(&file).unwrap();
                std::fs::write(&file, apply(&t, *op)).unwrap();
                if let DiskCheck::Ingested { .. } = s.file_changed().unwrap() {
                    ingested += 1;
                }
            }
            Step::Flush => {
                s.flush(agents[0]).unwrap();
            }
        }
    }
    let mismatches: Vec<String> = host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, AuditEvent::IdentityMismatch { .. }))
        .map(|e| format!("{e:?}"))
        .collect();
    mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (ingested, mismatches)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(192)
    ))]

    /// Every outside save's change at identity from the merge's hunks equals the full
    /// computation's, in repeated text, with unwritten agent edits.
    #[test]
    fn the_merges_hunks_are_the_change_at_identity(
        seed in proptest::collection::vec(0u8..3, 1..8),
        steps in proptest::collection::vec(step(), 1..24),
    ) {
        let (_, mismatches) = run(&seed, &steps);
        prop_assert!(mismatches.is_empty(), "{mismatches:?}");
    }
}

/// The run is not vacuous: outside saves are ingested and checked.
#[test]
fn outside_saves_are_ingested_and_checked() {
    let steps = vec![
        Step::Agent(0, (0, 1, 1)),
        Step::Outside((3, 0, 0)),
        Step::Outside((0, 2, 2)),
        Step::Flush,
        Step::Agent(1, (3, 1, 0)),
        Step::Outside((1, 1, 0)),
        Step::Outside((2, 0, 1)),
    ];
    let (ingested, mismatches) = run(&[0, 0, 1, 0], &steps);
    assert!(ingested >= 3, "{ingested}");
    assert!(mismatches.is_empty(), "{mismatches:?}");
}

/// Option B's whole-line sides, trimmed in ingest to the characters that change (v0.9.7):
/// a save that changes one word of a line an agent wrote leaves the agent's other lines and
/// words theirs, and a save over a line the agent also changed (unwritten) keeps both lines,
/// each its own; every save is checked at identity (`edit_check_every = 1`) and none differs.
#[test]
fn a_by_line_save_is_trimmed_and_a_keep_both_save_keeps_both_lines() {
    let dir = std::env::temp_dir().join(format!("k2mb-outside-trim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let file = dir.join("ws").join("a.md");
    std::fs::write(&file, "top\n").unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000),
        events: Mutex::new(Vec::new()),
    });
    let mut cfg = Config::default();
    cfg.edit_check_every = 1;
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), cfg);
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let s = mgr
        .open_file(Arc::new(MD), &key, &file, &dir.join("ws"))
        .unwrap();
    let a = s
        .join(
            Participant::new("a1", "A", "#1", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let text = || s.read(a).unwrap().text_by_root[0].1.clone();
    let author = |line: usize| s.blame(a, ROOT, None).unwrap().lines[line].author.clone();
    s.replace_text(a, ROOT, "top\nalpha beta gamma\nmid\nlast line\n")
        .unwrap();
    s.flush(a).unwrap();
    // by line: the file changes one word; only that word is the save's
    host.now.fetch_add(100, Ordering::SeqCst);
    std::fs::write(&file, "top\nalpha BETA gamma\nmid\nlast line\n").unwrap();
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert_eq!(text(), "top\nalpha BETA gamma\nmid\nlast line\n");
    assert_eq!(author(2), "a1", "the save left `mid` as it was");
    assert_eq!(author(3), "a1", "and `last line`");
    // keep both: the agent rewrites `mid` (not written back) and the file rewrites it too
    host.now.fetch_add(100, Ordering::SeqCst);
    s.replace_text(a, ROOT, "top\nalpha BETA gamma\nmid by agent\nlast line\n")
        .unwrap();
    host.now.fetch_add(100, Ordering::SeqCst);
    std::fs::write(&file, "top\nalpha BETA gamma\nmid by file\nlast line\n").unwrap();
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    let t = text();
    assert_eq!(
        t,
        "top\nalpha BETA gamma\nmid by agent\nmid by file\nlast line\n"
    );
    assert_eq!(
        author(4),
        "a1",
        "the agent's last line is still theirs: {t:?}"
    );
    let mismatches: Vec<String> = host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, AuditEvent::IdentityMismatch { .. }))
        .map(|e| format!("{e:?}"))
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:?}");
    drop(s);
    mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
