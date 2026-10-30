//! The outside-edit path, stateful: a session on a real file, driven by two participants'
//! edits, changes that move no byte (an edit and its undo, a delete and a retype, a rewrite
//! and back, a restore, a revert), `write --base`, flushes, ticks, restarts (a crash: the
//! manager dropped without a write-back) and outside edits (a random splice of the file).
//!
//! Properties, after every step:
//! - **an outside edit is kept, exactly:** when the doc's text is the file's text before it,
//!   the doc is the outside edit right after `file_changed`, and the file is the doc after
//!   the next write-back (the class of the v0.9.3 bug: a change that moved no byte left the
//!   disk replica on old elements, and the next outside edit was corrupted);
//! - an outside edit made while the doc has unwritten changes merges: afterwards the file
//!   and the doc agree after a flush;
//! - **a flush makes the file the doc,** and leaves the session clean;
//! - **a restart keeps the doc:** the text after reopening is the text before the crash;
//! - **`write --base` is the three-way merge:** applied, the doc is `merge3(base, ours,
//!   live)`; refused with conflicts exactly when that merge conflicts, and then nothing
//!   changed;
//! - undo, redo, revert and restore leave the doc a valid text the file can hold;
//! - ticks report no errors, and the kit never panics;
//! - feature `merge-rule` (a kit with the three-way ingest, v0.9.5): an outside edit made
//!   over unwritten changes is write --base's merge of the file's text on the synced text
//!   (on the kit's sides, `outside_sides`: whole lines since v0.9.7), with an open last
//!   line broken before the other side's line (`keepboth::clean_outside`), when that merge is
//!   clean, and keeps both sides by line when it conflicts (`crate::keepboth`: every line
//!   either side wrote, no line either deleted); clean or not, no line of the result holds
//!   both sides' changes and no two lines are joined (`keepboth::no_splice_no_join`); and a
//!   flush then leaves nothing pending;
//! - feature `echo-rule`: a save matching an earlier kit write counts as an echo only when
//!   nothing but the kit changed the file since.
//!
//! Run it with `-len_control=0` (a case needs a few dozen bytes before it has steps).

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arbitrary::{Arbitrary, Unstructured};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, DiskCheck, DiskState, Host, Participant, ParticipantKind, Rights,
    Session, SessionManager, Stay, TextFilePack, UndoOutcome, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::merge::{self, Merge};

use crate::{span, splice, Text, Tok};

const PACK: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

/// One fuzz case: the file's first content, then the steps.
#[derive(Debug)]
pub struct Case {
    pub seed: Vec<Line>,
    pub ops: Vec<Op>,
}

/// Lines from a small vocabulary, so text repeats.
#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum Line {
    A,
    B,
    Line3,
    Xy,
    Empty,
    Line3x,
    Kanji,
}

impl Line {
    fn as_str(self) -> &'static str {
        match self {
            Line::A => "a\n",
            Line::B => "b\n",
            Line::Line3 => "line 3\n",
            Line::Xy => "x y\n",
            Line::Empty => "\n",
            Line::Line3x => "line 3 x\n",
            Line::Kanji => "日 🎉\n",
        }
    }
}

/// A position as two fractions of the text's length (a span; equal: a point).
pub type At = (u16, u16);

#[derive(Debug, Clone)]
pub enum Op {
    /// A participant replaces a span with text.
    Edit {
        who: bool,
        at: At,
        ins: Text,
    },
    /// An edit and its undo (no byte moves).
    EditUndo {
        who: bool,
        at: At,
        ins: Text,
    },
    /// An edit, then a revert of it (no byte moves).
    EditRevert {
        who: bool,
        at: At,
        ins: Text,
    },
    /// Name a version, edit, restore it (no byte moves).
    EditRestore {
        who: bool,
        at: At,
        ins: Text,
    },
    Undo {
        who: bool,
    },
    Redo {
        who: bool,
    },
    /// Delete a span, then type it back (no byte moves).
    DeleteRetype {
        who: bool,
        at: At,
    },
    /// Replace a span, then set the whole text back (no byte moves).
    RewriteBack {
        who: bool,
        at: At,
        ins: Text,
    },
    /// Name the current version (a restore target).
    NameVersion {
        who: bool,
    },
    /// Restore one of the named versions.
    Restore {
        who: bool,
        pick: u8,
    },
    /// Revert one of the earlier changes.
    Revert {
        who: bool,
        pick: u8,
    },
    /// `write --base` against one of the versions read earlier.
    WriteBase {
        who: bool,
        pick: u8,
        at: At,
        ins: Text,
    },
    /// The host's clock moves on and it ticks (a write-back when the doc has been dirty long
    /// enough).
    Tick {
        ms: u16,
    },
    Flush {
        who: bool,
    },
    /// A crash: the manager is dropped without a write-back, and the doc reopened.
    Restart,
    /// A plain editor changes the file.
    Outside {
        at: At,
        ins: Text,
    },
    /// A plain editor saves the file unchanged.
    OutsideSame,
}

impl<'a> Arbitrary<'a> for Op {
    /// Weighted: outside edits and changes that move no byte are common, so their
    /// sequences (the bug class this target is for) are too.
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let who: bool = u.arbitrary()?;
        let pick: u8 = u.arbitrary()?;
        Ok(match u.int_in_range(0u8..=99)? {
            0..=19 => Op::Outside {
                at: u.arbitrary()?,
                ins: u.arbitrary()?,
            },
            20..=21 => Op::OutsideSame,
            22..=33 => Op::Edit {
                who,
                at: u.arbitrary()?,
                ins: u.arbitrary()?,
            },
            34..=40 => Op::EditUndo {
                who,
                at: u.arbitrary()?,
                ins: u.arbitrary()?,
            },
            41..=45 => Op::EditRevert {
                who,
                at: u.arbitrary()?,
                ins: u.arbitrary()?,
            },
            46..=50 => Op::EditRestore {
                who,
                at: u.arbitrary()?,
                ins: u.arbitrary()?,
            },
            51..=56 => Op::DeleteRetype {
                who,
                at: u.arbitrary()?,
            },
            57..=62 => Op::RewriteBack {
                who,
                at: u.arbitrary()?,
                ins: u.arbitrary()?,
            },
            63..=67 => Op::Undo { who },
            68..=70 => Op::Redo { who },
            71..=72 => Op::NameVersion { who },
            73..=75 => Op::Restore { who, pick },
            76..=78 => Op::Revert { who, pick },
            79..=83 => Op::WriteBase {
                who,
                pick,
                at: u.arbitrary()?,
                ins: u.arbitrary()?,
            },
            84..=89 => Op::Tick { ms: u.arbitrary()? },
            90..=95 => Op::Flush { who },
            _ => Op::Restart,
        })
    }
}

/// At most this many steps per case (a case reopens a store each restart: keep it quick).
pub const MAX_OPS: usize = 24;

impl<'a> Arbitrary<'a> for Case {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let n = u.int_in_range(1..=12)?;
        let mut seed = Vec::with_capacity(n);
        for _ in 0..n {
            seed.push(u.arbitrary()?);
        }
        let k = u.int_in_range(0..=MAX_OPS)?;
        let mut ops = Vec::with_capacity(k);
        for _ in 0..k {
            if u.is_empty() {
                break;
            }
            ops.push(u.arbitrary()?);
        }
        Ok(Case { seed, ops })
    }
}

// ---------- the host ----------

struct FuzzHost {
    now: AtomicU64,
}

impl Host for FuzzHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

/// A scratch folder (a store and a workspace), removed when dropped. Under `/dev/shm` when
/// there is one (a store commit per edit: keep it in memory).
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let root = ["/dev/shm"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let p = root.join(format!(
            "k2mb-fuzz-outside-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(p.join("ws")).expect("scratch folder");
        Scratch(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct World {
    // field order is drop order: the sessions and the manager before the folder
    s: Arc<Session>,
    conns: [ConnId; 2],
    mgr: SessionManager,
    store: Arc<DocStore>,
    host: Arc<FuzzHost>,
    ws: PathBuf,
    /// This case's file (its name is its doc's key).
    name: String,
    /// Versions read (and their text): `write --base` targets.
    reads: Vec<(u64, String)>,
    /// Named versions: restore targets.
    named: Vec<u64>,
    /// Changes made by each participant: revert targets.
    changes: Vec<u64>,
    log: Vec<String>,
    /// The file as this harness last saw it, and the kit's writes since the session opened
    /// (bytes, host time, whether an outside edit was ingested since): echo suppression
    /// ignores a save that brings one of them back.
    seen: String,
    kit_writes: Vec<(String, u64, bool)>,
}

/// Cases share a store (opening one costs more than a case): each case has its own file
/// and doc, and the store is replaced every [`CASES_PER_STORE`] cases.
struct Shared {
    store: Arc<DocStore>,
    ws: PathBuf,
    used: u32,
    _dir: Scratch,
}

const CASES_PER_STORE: u32 = 256;

thread_local! {
    static SHARED: std::cell::RefCell<Option<Shared>> = const { std::cell::RefCell::new(None) };
}

/// The shared store and workspace, and a fresh file name for this case.
fn shared() -> (Arc<DocStore>, PathBuf, String) {
    SHARED.with(|c| {
        let mut c = c.borrow_mut();
        if c.as_ref().is_none_or(|s| s.used >= CASES_PER_STORE) {
            // the old store (and its folder) go first
            *c = None;
            let dir = Scratch::new();
            let ws = fs::canonicalize(dir.0.join("ws")).expect("ws");
            let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).expect("store"));
            *c = Some(Shared {
                store,
                ws,
                used: 0,
                _dir: dir,
            });
        }
        let s = c.as_mut().expect("just made");
        s.used += 1;
        (s.store.clone(), s.ws.clone(), format!("c{}.md", s.used))
    })
}

fn config() -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c
}

fn key(name: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: name.into(),
    }
}

impl World {
    fn new(seed: &str) -> World {
        let (store, ws, name) = shared();
        fs::write(ws.join(&name), seed).expect("seed file");
        let host = Arc::new(FuzzHost {
            now: AtomicU64::new(1_000_000),
        });
        let mgr = SessionManager::new(store.clone(), host.clone(), config());
        let (s, conns) = open(&mgr, &ws, &name);
        World {
            s,
            conns,
            mgr,
            store,
            host,
            ws,
            name,
            reads: Vec::new(),
            named: Vec::new(),
            changes: Vec::new(),
            log: Vec::new(),
            seen: seed.to_owned(),
            kit_writes: Vec::new(),
        }
    }

    fn file(&self) -> String {
        String::from_utf8(fs::read(self.ws.join(&self.name)).expect("read the file"))
            .expect("the file is UTF-8")
    }

    fn conn(&self, who: bool) -> ConnId {
        self.conns[who as usize]
    }

    /// The doc's text (and the read becomes a `write --base` target).
    fn text(&mut self, who: bool) -> String {
        let r = self.s.read(self.conn(who)).expect("read");
        let t = r.text_by_root.into_iter().next().expect("one root").1;
        if self.reads.last().map(|x| x.0) != Some(r.version) {
            self.reads.push((r.version, t.clone()));
            if self.reads.len() > 8 {
                self.reads.remove(0);
            }
        }
        t
    }

    fn disk(&self) -> DiskState {
        self.s
            .status()
            .expect("status")
            .disk
            .expect("a file-backed session")
    }

    /// Note a write of the kit's: the file changed since this harness last saw it.
    fn note(&mut self) {
        let f = self.file();
        if f != self.seen {
            self.kit_writes.push((f.clone(), self.host.now_ms(), false));
            self.seen = f;
        }
    }

    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }

    fn tick(&self) {
        let r = self.mgr.tick().expect("tick");
        assert!(
            r.errors.is_empty(),
            "tick errors: {:?}\n{}",
            r.errors,
            self.trace()
        );
    }

    fn set(&mut self, who: bool, new: &str) -> Option<u64> {
        let r = self
            .s
            .edit_with(self.conn(who), ROOT, |_| Ok::<_, ()>(new.to_owned()))
            .ok()
            .flatten()?;
        self.changes.push(r.version);
        Some(r.version)
    }

    fn trace(&self) -> String {
        self.log.join("\n")
    }

    fn flush(&mut self, who: bool) {
        let out = self.s.flush(self.conn(who)).expect("flush");
        let doc = self.text(who);
        assert_eq!(
            self.file(),
            doc,
            "after a flush the file is the doc\n{}",
            self.trace()
        );
        assert_eq!(
            out.disk,
            Some(DiskState::Clean),
            "clean after a flush\n{}",
            self.trace()
        );
    }
}

fn open(mgr: &SessionManager, ws: &std::path::Path, name: &str) -> (Arc<Session>, [ConnId; 2]) {
    let s = mgr
        .open_file(Arc::new(PACK), &key(name), &ws.join(name), ws)
        .expect("open the file");
    let join = |pid: &str| {
        s.join(
            Participant::new(pid, pid, "#123456", ParticipantKind::Agent, "fuzz"),
            Stay::Held,
        )
        .expect("join")
        .conn
    };
    let conns = [join("agent-a"), join("agent-b")];
    (s, conns)
}

#[cfg_attr(not(feature = "merge-rule"), allow(dead_code))]
/// The keep-both checks so far (checks, regions, line texts checked, O4 dedupes, skipped),
/// printed every 1,000 checks.
fn keepboth_tally(st: &crate::keepboth::Stats) {
    use std::sync::atomic::AtomicUsize;
    static T: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];
    let add = |i: usize, n: usize| T[i].fetch_add(n, Ordering::Relaxed) + n;
    let n = add(0, 1);
    add(1, st.regions);
    add(2, st.exact);
    add(3, st.o4);
    add(4, st.skipped);
    if n % 1000 == 0 {
        let g = |i: usize| T[i].load(Ordering::Relaxed);
        eprintln!(
            "keepboth: {n} checks, {} regions; line texts checked {}, identical changes taken once {}, skipped {}",
            g(1),
            g(2),
            g(3),
            g(4)
        );
    }
}

/// Fixed cases, as readable inputs (the target runs them once per process, before its
/// first case): each found by the fuzzer or the soak, and fixed.
pub fn regressions() {
    outside_delete_then_retype_over_unwritten_edits();
}

/// The soak's outside delete-retype duplicate (v0.9.5; the outside save's sides are now
/// diffed on whole lines): unwritten live edits took out `p22:2150` and added two lines
/// after `p10:2205`; a plain editor deletes `p10:2205` and saves, then types it back and
/// saves (the bytes as before). No line may end up in the doc twice.
fn outside_delete_then_retype_over_unwritten_edits() {
    let base = "p05:2216\np22:2150\np10:2205\n";
    let live = "p05:2216\np10:2205\np23:2229\np25:2184\n";
    let deleted = "p05:2216\np22:2150\n";
    let mut w = World::new(base);
    w.log
        .push(format!("regression: seed {base:?}, live {live:?}"));
    w.set(false, live).expect("the live edit");
    for save in [deleted, base] {
        w.log.push(format!("  outside save {save:?}"));
        w.advance(100);
        fs::write(w.ws.join(&w.name), save).expect("an outside save");
        w.s.file_changed().expect("file_changed");
        let t = w.text(false);
        let mut seen = std::collections::HashSet::new();
        for l in t.lines() {
            assert!(
                seen.insert(l),
                "an outside delete and retype never yields a line twice: {l:?} in {t:?}\n{}",
                w.trace()
            );
        }
    }
    w.flush(false);
}

/// Run one case; panics on a violated property.
pub fn run(case: &Case) {
    let seed: String = case.seed.iter().map(|l| l.as_str()).collect();
    let mut w = World::new(&seed);
    w.log.push(format!("seed {seed:?}"));
    for op in &case.ops {
        w.log.push(format!("{op:?}"));
        step(&mut w, op);
        // after every step: the doc is a text the file can hold, and a read is consistent
        let _ = w.text(false);
    }
    // the end: a flush makes the file the doc
    w.flush(false);
}

fn ins_of(t: &Text) -> String {
    let s = t.render();
    if s.is_empty() {
        Tok::A.as_str().to_owned()
    } else {
        s
    }
}

fn step(w: &mut World, op: &Op) {
    match op {
        Op::Edit { who, at, ins } => {
            let t = w.text(*who);
            let new = splice(&t, span(&t, at.0, at.1), &ins.render());
            if new != t {
                w.set(*who, &new);
            }
        }
        Op::EditUndo { who, at, ins } => {
            let t = w.text(*who);
            let new = splice(&t, span(&t, at.0, at.1), &ins_of(ins));
            if new != t && w.set(*who, &new).is_some() {
                let _ = w.s.undo(w.conn(*who), 1);
            }
        }
        Op::EditRevert { who, at, ins } => {
            let t = w.text(*who);
            let new = splice(&t, span(&t, at.0, at.1), &ins_of(ins));
            if new != t {
                if let Some(v) = w.set(*who, &new) {
                    let _ = w.s.revert(w.conn(*who), v, false);
                }
            }
        }
        Op::EditRestore { who, at, ins } => {
            let t = w.text(*who);
            let new = splice(&t, span(&t, at.0, at.1), &ins_of(ins));
            let n = w.named.len();
            if new != t {
                if let Ok(v) = w.s.name_version(w.conn(*who), &format!("v{n}"), None) {
                    w.named.push(v);
                    if w.set(*who, &new).is_some() {
                        let _ = w.s.restore(w.conn(*who), v);
                    }
                }
            }
        }
        Op::Undo { who } => {
            let _ = w.s.undo(w.conn(*who), 1);
        }
        Op::Redo { who } => {
            let _ = w.s.redo(w.conn(*who), 1);
        }
        Op::DeleteRetype { who, at } => {
            let t = w.text(*who);
            let (lo, mut hi) = span(&t, at.0, at.1);
            if lo == hi {
                hi = t[lo..].chars().next().map_or(lo, |c| lo + c.len_utf8());
            }
            if lo == hi {
                return;
            }
            let gone = t[lo..hi].to_owned();
            if w.set(*who, &splice(&t, (lo, hi), "")).is_some() {
                let _ = w.s.edit_with(w.conn(*who), ROOT, |cur| {
                    Ok::<_, ()>(format!("{}{gone}{}", &cur[..lo], &cur[lo..]))
                });
            }
        }
        Op::RewriteBack { who, at, ins } => {
            let t = w.text(*who);
            let mut new = splice(&t, span(&t, at.0, at.1), &ins_of(ins));
            if new == t {
                new.push('Z');
            }
            if w.set(*who, &new).is_some() {
                w.set(*who, &t);
            }
        }
        Op::NameVersion { who } => {
            let n = w.named.len();
            if let Ok(v) = w.s.name_version(w.conn(*who), &format!("v{n}"), None) {
                w.named.push(v);
            }
        }
        Op::Restore { who, pick } => {
            if !w.named.is_empty() {
                let v = w.named[*pick as usize % w.named.len()];
                if let Ok(r) = w.s.restore(w.conn(*who), v) {
                    w.changes.push(r.version);
                }
            }
        }
        Op::Revert { who, pick } => {
            if !w.changes.is_empty() {
                let c = w.changes[*pick as usize % w.changes.len()];
                if let Ok(UndoOutcome::Applied { result, .. }) = w.s.revert(w.conn(*who), c, false)
                {
                    w.changes.push(result.version);
                }
            }
        }
        Op::WriteBase { who, pick, at, ins } => {
            let live = w.text(*who);
            let (base_v, base) = w.reads[*pick as usize % w.reads.len()].clone();
            let ours = splice(&base, span(&base, at.0, at.1), &ins.render());
            let out = w.s.write_base(w.conn(*who), ROOT, base_v, &ours);
            let after = w.text(*who);
            match (out, merge::merge3(&base, &ours, &live)) {
                (Ok(WriteOutcome::Applied(r)), Merge::Clean(c)) => {
                    assert_eq!(after, c.text, "write --base is merge3\n{}", w.trace());
                    if r.version != 0 {
                        w.changes.push(r.version);
                    }
                }
                (Ok(WriteOutcome::Conflicts(_)), Merge::Conflicts(_)) => {
                    assert_eq!(
                        after,
                        live,
                        "a refused write --base changes nothing\n{}",
                        w.trace()
                    );
                }
                (Ok(WriteOutcome::BaseExpired), _) | (Err(_), _) => {}
                (out, m) => panic!(
                    "write --base disagrees with merge3: {out:?} vs {m:?}\n{}",
                    w.trace()
                ),
            }
        }
        Op::Tick { ms } => {
            w.advance(*ms as u64 * 4);
            w.tick();
        }
        Op::Flush { who } => w.flush(*who),
        Op::Restart => {
            let before = w.text(false);
            // a crash: no write-back; the old manager and session go away
            w.mgr = SessionManager::new(w.store.clone(), w.host.clone(), config());
            let (s, conns) = open(&w.mgr, &w.ws, &w.name);
            w.s = s;
            w.conns = conns;
            w.reads.clear();
            // the echo list is memory only
            w.note();
            w.kit_writes.clear();
            let after = w.text(false);
            assert_eq!(after, before, "a restart keeps the doc\n{}", w.trace());
        }
        Op::Outside { at, ins } => {
            let doc = w.text(false);
            let file = w.file();
            let mut edited = splice(&file, span(&file, at.0, at.1), &ins.render());
            if edited == file {
                edited.push_str("Z\n");
            }
            w.log.push(format!(
                "  doc {doc:?}\n  file {file:?}\n  edited {edited:?}"
            ));
            w.note();
            w.advance(1_000);
            // Echo suppression (Config::own_write_ttl_ms): a save that brings back bytes the
            // kit wrote within the window may be taken for an echo. Default: such a save is
            // only logged. Feature `echo-rule`: it is an echo only when nothing but the kit
            // changed the file since that write.
            let now = w.host.now_ms();
            let window = |at: u64| now.saturating_sub(at) <= config().own_write_ttl_ms;
            let echo = if cfg!(feature = "echo-rule") {
                w.kit_writes
                    .iter()
                    .any(|(b, at, outside)| *b == edited && window(*at) && !outside)
            } else {
                w.kit_writes
                    .iter()
                    .any(|(b, at, _)| *b == edited && window(*at))
            };
            fs::write(w.ws.join(&w.name), &edited).expect("the outside edit");
            w.seen = edited.clone();
            let check = w.s.file_changed().expect("file_changed");
            if echo && matches!(check, DiskCheck::OwnWrite) {
                w.log
                    .push("  taken for an echo (a recent kit write)".into());
                w.advance(2_000);
                w.tick();
                w.flush(false);
                w.note();
                return;
            }
            // a save of exactly the live text: nothing to merge (the kit may say so); with
            // the three-way ingest, any merge that leaves the live text as it is (checked by
            // merge-rule below)
            let same_as_live = matches!(check, DiskCheck::Unchanged)
                && (edited == doc || (cfg!(feature = "merge-rule") && doc != file));
            assert!(
                matches!(check, DiskCheck::Ingested { .. }) || same_as_live,
                "an outside edit is ingested: {check:?}\n{}",
                w.trace()
            );
            if same_as_live {
                assert_eq!(
                    w.text(false),
                    doc,
                    "Unchanged, so the doc is as it was\n{}",
                    w.trace()
                );
            }
            for k in &mut w.kit_writes {
                k.2 = true;
            }
            // Feature `merge-rule`: an outside edit made while the doc has unwritten changes
            // is the three-way merge of the synced bytes (the file before it), the live text
            // and the new file, when that merge is clean (either side's end-of-text append
            // may come first); a flush then leaves nothing pending (checked below).
            #[cfg(feature = "merge-rule")]
            if doc != file {
                // the sides as the kit's three-way ingest takes them (`outside_sides`: whole
                // lines of the closed texts, checked against the kit's side diff), then the
                // whole oracle (`crate::keepboth::check_outside`): no splice or join, the
                // exact lines (nothing lost, nothing deleted back), a clean merge exactly, a
                // conflicting one kept both by line; (iv) is the flush below
                let got = w.text(false);
                let (a, b) = merge::outside_sides(&file, &edited, &doc);
                match crate::keepboth::check_outside(&file, &edited, &doc, &a, &b, &got) {
                    Ok(crate::keepboth::Outcome::KeptBoth(st)) => keepboth_tally(&st),
                    Ok(crate::keepboth::Outcome::Clean) => {}
                    Err(e) => panic!(
                        "an outside edit over unwritten changes: {e}\n  got {got:?}\n{}",
                        w.trace()
                    ),
                }
            }
            if doc == file {
                // the doc was the file: the outside edit is the doc, exactly
                let got = w.text(false);
                assert_eq!(
                    got,
                    edited,
                    "the outside edit is kept, exactly\n{}",
                    w.trace()
                );
            }
            w.advance(2_000);
            w.tick();
            w.flush(false);
            if doc == file {
                assert_eq!(
                    w.file(),
                    edited,
                    "the file is the outside edit\n{}",
                    w.trace()
                );
            }
        }
        Op::OutsideSame => {
            let file = w.file();
            let doc = w.text(false);
            w.advance(1_000);
            fs::write(w.ws.join(&w.name), &file).expect("a save");
            w.seen = file.clone();
            let _ = w.s.file_changed().expect("file_changed");
            assert_eq!(
                w.text(false),
                doc,
                "an unchanged save changes nothing\n{}",
                w.trace()
            );
        }
    }
    let _ = w.disk();
    w.note();
}
