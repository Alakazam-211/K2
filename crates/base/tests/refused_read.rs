//! A read verb that fails after the doc was read gave the caller nothing, so it isn't their read:
//! the next read still says who changed the doc since they last read it, and others see no
//! activity. Text docs and a text pack's own read verb; the grid's are in `tests/grid_lines.rs`.
//! Public API only, but for the planted fault: `MPC_LINE_MUTANT=refused-read-is-a-read` (the setting
//! echoed), caught by both `a_refused_read_` tests.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, Host, Pack, PackContent, PackError, Participant,
    ParticipantKind, Rights, Session, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{PackVerb, StructEdit, ToolError};
use k2_mpc_base::Schema;

/// Runs `f` with the planted fault when the environment asks for it.
fn under_line_mutant<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_line_mutant, LineMutant};
    let m = std::env::var("MPC_LINE_MUTANT");
    println!("MPC_LINE_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("refused-read-is-a-read") => with_line_mutant(LineMutant::RefusedReadIsARead, f),
        // the other faults are tests/grid_lines.rs's
        _ => f(),
    }
}

const ROOT: &str = TextFilePack::ROOT;
const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

/// A text pack with one read verb of its own, `sections`, which refuses when the verb runs (a
/// pack's address that names nothing, say).
struct WithReadVerb;

fn sections(_: &[String]) -> Result<StructEdit, ToolError> {
    Ok(Box::new(|_ctx| Err(ToolError::usage("no such section"))))
}

const VERBS: &[PackVerb] = &[PackVerb::structured_read(
    "sections",
    "sections",
    &[PersonAction::ReadAll],
    &[0],
    sections,
)];

impl Pack for WithReadVerb {
    fn schema(&self) -> Schema {
        MD.schema()
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        MD.decode(bytes)
    }
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        MD.encode(content, codec)
    }
    fn normalize_input<'a>(&self, root: &str, text: &'a str) -> Cow<'a, str> {
        MD.normalize_input(root, text)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
}

struct H;
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::editor()
    }
    fn now_ms(&self) -> u64 {
        1_000_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    s: Arc<Session>,
    reader: ConnId,
    writer: ConnId,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx() -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-refused-read-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H), Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: "r.md".into(),
    };
    let s = mgr
        .open(Arc::new(WithReadVerb), &key, Some(b"one\ntwo\n"))
        .unwrap();
    let join = |pid: &str, kind| {
        let p = Participant::new(pid, pid.to_uppercase(), "#123456", kind, "cli");
        s.join(p, Stay::Held).unwrap().conn
    };
    let reader = join("reader", ParticipantKind::Agent);
    let writer = join("writer", ParticipantKind::Person);
    Fx {
        dir,
        s,
        reader,
        writer,
    }
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// `(verb, stdin)`: each fails only when it runs, after the doc was read.
const REFUSED: &[(&[&str], &[u8])] = &[
    // no regex to reuse
    (&["sed", "s//x/"], b""),
    // the old text isn't UTF-8
    (&["diff"], b"\xff"),
    // a pack's own read verb
    (&["sections"], b""),
];

fn who_changed_it_after(f: &Fx, between: impl FnOnce(&Fx)) -> Vec<String> {
    f.s.run_tool(f.reader, ROOT, &argv(&["cat"]), b"").unwrap();
    f.s.edit_with(f.writer, ROOT, |t| Ok::<_, String>(t.replace("one", "ONE")))
        .unwrap();
    between(f);
    let o = f.s.run_tool(f.reader, ROOT, &argv(&["cat"]), b"").unwrap();
    o.fence.unwrap().contributors
}

/// The controls: a read that works says who changed the doc, and then there's no one new; and it
/// is activity. So the tests below would see a real read.
#[test]
fn a_read_that_works_is_a_read() {
    under_line_mutant(|| {
        let f = fx();
        assert_eq!(who_changed_it_after(&f, |_| {}), ["writer"]);
        let again = f.s.run_tool(f.reader, ROOT, &argv(&["cat"]), b"").unwrap();
        assert!(again.fence.unwrap().contributors.is_empty());
        let f = fx();
        let before = f.s.activity(f.reader, 50).unwrap().len();
        f.s.run_tool(f.reader, ROOT, &argv(&["cat"]), b"").unwrap();
        assert_eq!(f.s.activity(f.reader, 50).unwrap().len(), before + 1);
    })
}

#[test]
fn a_refused_read_keeps_who_changed_the_doc() {
    under_line_mutant(|| {
        let mut wrong = Vec::new();
        for (a, stdin) in REFUSED {
            let f = fx();
            let got = who_changed_it_after(&f, |f| {
                let r = f.s.run_tool(f.reader, ROOT, &argv(a), stdin);
                assert!(r.is_err(), "{a:?}: not refused: {r:?}");
            });
            println!("{a:?}: contributors {got:?}");
            if got != ["writer"] {
                wrong.push(format!("{a:?}: {got:?}"));
            }
        }
        assert!(wrong.is_empty(), "lost who changed the doc: {wrong:?}");
    })
}

#[test]
fn a_refused_read_is_no_activity() {
    under_line_mutant(|| {
        let mut wrong = Vec::new();
        for (a, stdin) in REFUSED {
            let f = fx();
            let before = f.s.activity(f.reader, 50).unwrap().len();
            let r = f.s.run_tool(f.reader, ROOT, &argv(a), stdin);
            assert!(r.is_err(), "{a:?}: not refused: {r:?}");
            let after = f.s.activity(f.reader, 50).unwrap().len();
            println!("{a:?}: activity {before} -> {after}");
            if after != before {
                wrong.push(format!("{a:?}: {before} -> {after}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "a refused read logged activity: {wrong:?}"
        );
    })
}
