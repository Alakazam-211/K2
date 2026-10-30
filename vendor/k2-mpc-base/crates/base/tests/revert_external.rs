//! A revert of an outside save puts back the roots the save removed (undo spec U23; fixed in
//! v0.9.7.2). Up to v0.9.7.1 an outside save's revert record kept no roots, so its revert put
//! the text back as fresh elements: blame credited the reverter, and O4a, I1 and U10a, which
//! read roots, couldn't see that the text was already live elsewhere (the soak's seed 1, call
//! 43,002: a line twice). Each case: the revert, an undo of the revert, a redo, and the same
//! with a restart between the save and the revert; roots read with `char_blame`, `restored_by`
//! with blame. Case C, an ordinary edit reverted, is the control: it always kept its roots.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{with_prov_mutant, ProvMutant};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay, TextFilePack, UndoOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        let mut r = Rights::full();
        r.owner = who.pid == "olive";
        r
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

const ROOT: &str = TextFilePack::ROOT;
type Root = (u64, u32);

struct Doc {
    dir: std::path::PathBuf,
    file: std::path::PathBuf,
    mgr: SessionManager,
    s: Arc<Session>,
    sam: ConnId,
    olive: ConnId,
}

fn run(s: &Session, c: ConnId, argv: &[&str], stdin: &str) -> u64 {
    let argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
    let o = s
        .run_tool(c, ROOT, &argv, stdin.as_bytes())
        .unwrap_or_else(|e| panic!("{argv:?}: {e:?}"));
    assert!(o.changed.is_some(), "{argv:?}: exit {}", o.exit);
    o.version
}

fn applied(o: UndoOutcome) {
    assert!(matches!(o, UndoOutcome::Applied { .. }), "{o:?}");
}

fn open(
    dir: &std::path::Path,
    file: &std::path::Path,
) -> (SessionManager, Arc<Session>, ConnId, ConnId) {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(
        store,
        Arc::new(H(AtomicU64::new(1_000_000))),
        Config::default(),
    );
    let key = DocKey {
        space: "ws".into(),
        path: "notes.md".into(),
    };
    let s = mgr
        .open_file(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &key,
            file,
            &dir.join("ws"),
        )
        .unwrap();
    let join = |pid: &str| {
        s.join(
            Participant::new(pid, pid, "#1", ParticipantKind::Person, "app"),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let (sam, olive) = (join("sam"), join("olive"));
    (mgr, s, sam, olive)
}

impl Doc {
    fn new(label: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-revext-{}-{label}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        let file = dir.join("ws/notes.md");
        std::fs::write(&file, "a\nc\n").unwrap();
        let (mgr, s, sam, olive) = open(&dir, &file);
        Self {
            dir,
            file,
            mgr,
            s,
            sam,
            olive,
        }
    }

    fn text(&self) -> String {
        self.s.read(self.olive).unwrap().text_by_root[0].1.clone()
    }

    /// An outside save of `text` (a whole-text replacement of the file the kit wrote, through
    /// the real ingest); returns the change's id.
    fn outside_save(&self, text: &str) -> u64 {
        self.s.flush(self.olive).unwrap();
        std::fs::write(&self.file, text).unwrap();
        self.s.file_changed().unwrap();
        self.s.read(self.olive).unwrap().version
    }

    /// The roots of the live `b` line's characters (its text and break).
    fn b_roots(&self) -> Vec<Root> {
        let cb = self.s.char_blame(ROOT).unwrap();
        let text = self.text();
        let at = text.find("b\n").expect("b is live");
        let mut pos = 0;
        let mut out = Vec::new();
        for c in &cb {
            if pos >= at && pos < at + 2 {
                out.push((c.root_client, c.root_clock));
            }
            pos += c.ch.len_utf8();
        }
        out
    }

    /// I1: no root is live twice.
    fn i1(&self) {
        let cb = self.s.char_blame(ROOT).unwrap();
        let mut seen = std::collections::HashSet::new();
        for c in &cb {
            assert!(
                seen.insert((c.root_client, c.root_clock)),
                "I1: a root live twice: {cb:?}"
            );
        }
    }

    fn b_restored_by(&self) -> Option<String> {
        let b = self.s.blame(self.olive, ROOT, None).unwrap();
        b.lines
            .iter()
            .find(|l| l.text == "b")
            .and_then(|l| l.restored_by.clone())
    }

    /// A restart: the session released, the store reopened, everyone back.
    fn restart(self) -> Self {
        let Doc {
            dir, file, mgr, s, ..
        } = self;
        drop(s);
        mgr.release().unwrap();
        drop(mgr);
        let (mgr, s, sam, olive) = open(&dir, &file);
        Doc {
            dir,
            file,
            mgr,
            s,
            sam,
            olive,
        }
    }

    fn finish(self) {
        drop(self.s);
        self.mgr.release().unwrap();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// How `b` is taken out before the revert.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// (A) a typed line, taken out by an outside save
    Typed,
    /// (B) a line put back by an undo (an element whose root is another), taken out by an
    /// outside save: the soak's shape
    PutBack,
    /// (C) the control: a typed line, taken out by an ordinary edit
    Edit,
}

/// One case: `b` taken out (by `shape`), restarted or not, the owner's revert of that change
/// (all or `--partial`), then the revert undone and redone. At each step, `b`'s roots are the
/// roots it had before it was taken out, blame credits the reverter, and I1 holds.
fn case(shape: Shape, partial: bool, restart: bool) {
    let mut d = Doc::new(&format!("{shape:?}-{partial}-{restart}"));
    run(&d.s, d.sam, &["insert", "--line", "2"], "b\n");
    if let Shape::PutBack = shape {
        run(&d.s, d.olive, &["delete", "--lines", "2,2"], "");
        applied(d.s.undo(d.olive, 1).unwrap());
    }
    let roots = d.b_roots();
    let change = match shape {
        Shape::Edit => run(&d.s, d.sam, &["delete", "--lines", "2,2"], ""),
        _ => d.outside_save("a\nc\n"),
    };
    assert_eq!(d.text(), "a\nc\n");
    if restart {
        d = d.restart();
    }
    let what = format!("{shape:?}, partial {partial}, restart {restart}");
    applied(d.s.revert(d.olive, change, partial).unwrap());
    assert_eq!(d.text(), "a\nb\nc\n", "{what}");
    assert_eq!(
        d.b_roots(),
        roots,
        "{what}: the revert put back b's roots (U23)"
    );
    assert_eq!(d.b_restored_by().as_deref(), Some("olive"), "{what}: blame");
    d.i1();
    // the revert undone: b goes, its roots with it
    applied(d.s.undo(d.olive, 1).unwrap());
    assert_eq!(d.text(), "a\nc\n", "{what}: undo of the revert");
    d.i1();
    // and redone: b's roots again
    applied(d.s.redo(d.olive, 1).unwrap());
    assert_eq!(d.text(), "a\nb\nc\n", "{what}: redo");
    assert_eq!(
        d.b_roots(),
        roots,
        "{what}: the redo put back b's roots (U23)"
    );
    assert_eq!(
        d.b_restored_by().as_deref(),
        Some("olive"),
        "{what}: blame after the redo"
    );
    d.i1();
    d.finish();
}

fn all(shape: Shape) {
    for partial in [false, true] {
        for restart in [false, true] {
            case(shape, partial, restart);
        }
    }
}

/// (A) Failed on v0.9.7.1: the revert put `b` back as fresh text, credited to the reverter.
#[test]
fn a_revert_of_an_outside_save_puts_back_the_roots_it_removed() {
    all(Shape::Typed);
}

/// (B) Failed on v0.9.7.1 (the soak's seed 1): the root of a line an undo had put back was
/// lost too.
#[test]
fn a_revert_of_an_outside_save_puts_back_a_put_back_lines_root() {
    all(Shape::PutBack);
}

/// (C) The control: a revert of an ordinary edit always kept its roots.
#[test]
fn a_revert_of_an_edit_puts_back_its_roots() {
    all(Shape::Edit);
}

/// The planted fault (v0.9.7.1's record without roots, `ProvMutant::OutsideSaveKeepsNoRoots`)
/// fails cases A and B; without it they pass.
#[test]
fn the_mutant_that_drops_the_roots_is_caught() {
    for shape in [Shape::Typed, Shape::PutBack] {
        let r = std::panic::catch_unwind(|| {
            with_prov_mutant(ProvMutant::OutsideSaveKeepsNoRoots, || {
                case(shape, false, false)
            })
        });
        assert!(r.is_err(), "{shape:?}: the planted fault went unseen");
    }
    // without it, the same case passes
    case(Shape::Typed, false, false);
}
