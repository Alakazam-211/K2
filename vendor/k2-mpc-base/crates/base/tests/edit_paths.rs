//! The differential test of the engine's edit path (v0.9.3).
//!
//! Every edit the kit makes (an agent's or a person's verb, `write --base`, undo, redo,
//! revert, restore, an outside edit, an import) becomes an engine change through one path
//! ([`k2_mpc_base::differential::Path`]). Today's is the throwaway replica: the doc's full
//! current state applied to a fresh engine doc with the editor's client ID, edited, encoded and
//! dropped. A new path must give **byte-identical** results. Two drivers run the same seeded
//! random sequence of operations on two sides, one per path, and compare at **every step**:
//!
//! - **Doc level** ([`doc_paths_agree`]): agent edits (multi-hunk, two roots, multi-unit
//!   characters), people's updates from their own engine clients (some typed concurrently,
//!   on an out-of-date copy), and reloads from the encoded state. Compared: each update (or
//!   error), the full encoded state and state vector after each edit, the texts.
//! - **Session level** ([`session_paths_agree`]): people and agents in a file-backed session
//!   doing edits, multi-hunk edits, `write --base` from older reads, undo / redo (all,
//!   `--partial`, `--drop`), revert (`--partial` too), restore, named versions, purge (changes,
//!   a text, a participant), outside edits to the file, flushes, ticks, leaves and rejoins,
//!   and full restarts from the store. Compared: every call's outcome, every engine edit it
//!   made (update, state, state vector), the text, version, blame and log.
//!
//! Client IDs are minted from a seed on both sides (the kit's are random), so two sessions
//! driven alike hold the same IDs. Each driver must also **catch** a wrong path: against the
//! mutants (hunks applied front to back; a stale replica kept per client) it reports a
//! difference.
//!
//! The shadow mode ([`Side::shadowed_by`]) checks each edit inside one run instead of two: the
//! soak's `SOAK_SHADOW=1`. [`shadow_mode_checks_each_edit`] covers it here.
//!
//! Sizes: `EDIT_PATHS_SEEDS` (default 24) sequences of `EDIT_PATHS_STEPS` (default 150) at doc
//! level; `EDIT_PATHS_SESSION_SEEDS` (default 4) of `EDIT_PATHS_SESSION_STEPS` (default 150) at
//! session level. A node runs more:
//!
//! ```sh
//! EDIT_PATHS_SEEDS=2000 EDIT_PATHS_SESSION_SEEDS=200 \
//!   cargo test -p k2-mpc-base --release --test edit_paths -- --nocapture
//! ```

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{with_side, EditRecord, Path, Side};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, LogQuery, Participant, ParticipantKind, PurgeRequest,
    PurgeScope, Rights, Session, SessionManager, Stay, TextFilePack, UndoMode, UndoRequest,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::{Doc, DocError, Edit, Kind, Schema};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{GetString, OffsetKind, Options, ReadTxn, Text, Transact, Update};

/// The path under test (v0.9.3's, on the doc itself as the client), against the throwaway
/// replica ([`Path::Throwaway`], every edit's path before v0.9.3).
const CANDIDATE: Path = Path::MainDoc;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ---------- random edits ----------

const PIECES: &[&str] = &[
    "a", "line\n", "🎉", "日本", "x y z\n", "é", "\n", "αβ", "word ", "-- ",
];

fn insert_text(rng: &mut SmallRng) -> String {
    (0..rng.gen_range(1..4))
        .map(|_| PIECES[rng.gen_range(0..PIECES.len())])
        .collect()
}

fn boundaries(t: &str) -> Vec<usize> {
    t.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(t.len()))
        .collect()
}

/// 1 to 4 valid hunks against `t`: sorted, not overlapping, never two insertions at one place.
fn random_hunks(rng: &mut SmallRng, t: &str) -> Vec<Hunk> {
    let b = boundaries(t);
    let k = rng.gen_range(1..=4);
    let mut cuts: Vec<usize> = (0..2 * k).map(|_| b[rng.gen_range(0..b.len())]).collect();
    cuts.sort_unstable();
    let mut out: Vec<Hunk> = Vec::new();
    for pair in cuts.chunks(2) {
        let (start, end) = (pair[0], pair[1]);
        let insert = if rng.gen_bool(0.7) {
            insert_text(rng)
        } else {
            String::new()
        };
        if start == end && insert.is_empty() {
            continue;
        }
        if let Some(p) = out.last() {
            if start < p.end || (start == end && p.start == p.end && p.start == start) {
                continue;
            }
        }
        out.push(Hunk { start, end, insert });
    }
    if out.is_empty() {
        out.push(Hunk {
            start: t.len(),
            end: t.len(),
            insert: insert_text(rng),
        });
    }
    out
}

/// A first difference between the two sides: the step and what differed.
type Diverged = (usize, String);

fn same<T: PartialEq + std::fmt::Debug>(
    step: usize,
    what: &str,
    a: T,
    b: T,
) -> Result<(), Diverged> {
    if a == b {
        Ok(())
    } else {
        Err((
            step,
            format!("{what}:\n  throwaway: {a:?}\n  candidate: {b:?}"),
        ))
    }
}

thread_local! {
    /// This thread's state comparisons across the two sides: (compared, equal in content
    /// but not in bytes).
    static STATES: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
}

/// The two sides' states (v2) after a step: equal bytes, or else the same content (state
/// vector, delete set, texts: [`Doc::same_content`]). Two docs fed the same changes may split
/// equal content into blocks differently upstream, so state equality across docs is content
/// equality (k2-yrs k2.4's note); updates stay byte for byte. Counts the content-only ones.
fn same_state(
    step: usize,
    what: &str,
    schema: Schema,
    a: Option<Vec<u8>>,
    b: Option<Vec<u8>>,
) -> Result<(), Diverged> {
    STATES.with(|c| c.set((c.get().0 + 1, c.get().1)));
    if a == b {
        return Ok(());
    }
    if let (Some(x), Some(y)) = (&a, &b) {
        if let (Ok(dx), Ok(dy)) = (Doc::load(schema, x), Doc::load(schema, y)) {
            if dx.same_content(&dy) {
                STATES.with(|c| c.set((c.get().0, c.get().1 + 1)));
                return Ok(());
            }
        }
    }
    same(step, what, a, b)
}

/// This thread's (states compared, equal in content only) since the last call.
fn take_states() -> (u64, u64) {
    STATES.with(|c| c.replace((0, 0)))
}

fn same_edits(
    step: usize,
    schema: Schema,
    a: Vec<EditRecord>,
    b: Vec<EditRecord>,
) -> Result<(), Diverged> {
    same(step, "engine edits made", a.len(), b.len())?;
    for (k, (x, y)) in a.into_iter().zip(b).enumerate() {
        same(step, &format!("edit {k}: client"), x.client, y.client)?;
        same(step, &format!("edit {k}: update"), x.update, y.update)?;
        same(
            step,
            &format!("edit {k}: state vector after"),
            x.state_vector,
            y.state_vector,
        )?;
        same_state(
            step,
            &format!("edit {k}: state after"),
            schema,
            x.state,
            y.state,
        )?;
    }
    Ok(())
}

// ---------- the doc-level driver ----------

const TWO: Schema = Schema {
    name: "two",
    roots: &[("body", Kind::Text), ("notes", Kind::Text)],
};
const ROOTS: [&str; 2] = ["body", "notes"];
const AGENTS: [u64; 4] = [1001, 1002, 1003, 1004];
const PEOPLE: [u64; 3] = [2001, 2002, 2003];

/// A person's own engine client (the Yjs stand-in: a raw Yrs doc with their client ID).
fn person_doc(client: u64) -> yrs::Doc {
    yrs::Doc::with_options(Options {
        client_id: yrs::block::ClientID::new(client),
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    })
}

struct DocSide {
    side: Side,
    doc: Doc,
    people: HashMap<u64, yrs::Doc>,
}

impl DocSide {
    fn new(path: Path) -> Self {
        Self {
            side: Side::new(path).recording(),
            doc: Doc::new(TWO).unwrap(),
            people: PEOPLE.iter().map(|c| (*c, person_doc(*c))).collect(),
        }
    }

    /// Bring person `c`'s client up to the doc (sync step 2).
    fn sync_person(&mut self, c: u64) {
        let p = &self.people[&c];
        let sv = p.transact().state_vector().encode_v1();
        let u = self.doc.encode_diff(&sv).unwrap();
        p.transact_mut()
            .apply_update(Update::decode_v1(&u).unwrap())
            .unwrap();
    }
}

#[derive(Debug, Clone)]
enum DocOp {
    /// An agent's edit (one change, both roots possible).
    Agent(u64, Vec<Edit>),
    /// A person types on their own client (synced first, or not: a concurrent edit) and the
    /// doc applies the update: `(root, byte offset in the client's text, delete bytes,
    /// insert)`.
    Person {
        client: u64,
        sync: bool,
        root: &'static str,
        at: usize,
        del: usize,
        insert: String,
    },
    Reload,
}

fn doc_op(rng: &mut SmallRng, s: &DocSide) -> DocOp {
    match rng.gen_range(0..100) {
        0..=59 => {
            let client = AGENTS[rng.gen_range(0..AGENTS.len())];
            let mut edits = Vec::new();
            for root in ROOTS {
                if edits.is_empty() || rng.gen_bool(0.3) {
                    let t = s.doc.text(root).unwrap();
                    edits.push(Edit::Text {
                        root: root.into(),
                        hunks: random_hunks(rng, &t),
                    });
                }
            }
            if rng.gen_bool(0.5) {
                edits.reverse();
            }
            DocOp::Agent(client, edits)
        }
        60..=94 => {
            let client = PEOPLE[rng.gen_range(0..PEOPLE.len())];
            let root = ROOTS[rng.gen_range(0..2)];
            let sync = rng.gen_bool(0.7);
            // positions are drawn against the client's text after the sync it will do
            let t = if sync {
                s.doc.text(root).unwrap()
            } else {
                let p = &s.people[&client];
                let r = p.get_or_insert_text(root);
                let txn = p.transact();
                r.get_string(&txn)
            };
            let b = boundaries(&t);
            let i = rng.gen_range(0..b.len());
            let j = (i + rng.gen_range(0..4)).min(b.len() - 1);
            let insert = if rng.gen_bool(0.8) || i == j {
                insert_text(rng)
            } else {
                String::new()
            };
            DocOp::Person {
                client,
                sync,
                root,
                at: b[i],
                del: b[j] - b[i],
                insert,
            }
        }
        _ => DocOp::Reload,
    }
}

/// Run one op on one side: the update the doc made or took (or the error).
fn run_doc_op(s: &mut DocSide, op: &DocOp) -> Result<Option<Vec<u8>>, DocError> {
    match op {
        DocOp::Agent(client, edits) => {
            let doc = &mut s.doc;
            with_side(&mut s.side, || doc.edit_as(*client, edits))
        }
        DocOp::Person {
            client,
            sync,
            root,
            at,
            del,
            insert,
        } => {
            if *sync {
                s.sync_person(*client);
            }
            let p = &s.people[client];
            let r = p.get_or_insert_text(*root);
            let u = {
                let mut txn = p.transact_mut();
                let t = r.get_string(&txn);
                let at16 = t[..*at].encode_utf16().count() as u32;
                let del16 = t[*at..*at + *del].encode_utf16().count() as u32;
                if del16 > 0 {
                    r.remove_range(&mut txn, at16, del16);
                }
                if !insert.is_empty() {
                    r.insert(&mut txn, at16, insert);
                }
                txn.encode_update_v1()
            };
            s.doc.apply_client_update(&u, *client).map(|_| Some(u))
        }
        DocOp::Reload => {
            let state = s.doc.encode_state()?;
            s.doc = Doc::load(TWO, &state)?;
            Ok(None)
        }
    }
}

/// One seeded sequence through both paths; the first difference, if any.
fn doc_sequence(seed: u64, steps: usize, candidate: Path) -> Result<usize, Diverged> {
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut a = DocSide::new(Path::Throwaway);
    let mut b = DocSide::new(candidate);
    let mut agent_edits = 0;
    for step in 0..steps {
        let op = doc_op(&mut rng, &a);
        let ra = run_doc_op(&mut a, &op);
        let rb = catch_unwind(AssertUnwindSafe(|| run_doc_op(&mut b, &op)))
            .map_err(|_| (step, format!("the candidate panicked on {op:?}")))?;
        same(step, &format!("{op:?}: result"), &ra, &rb)?;
        if let DocOp::Agent(..) = op {
            agent_edits += usize::from(matches!(ra, Ok(Some(_))));
        }
        same_edits(step, TWO, a.side.take_edits(), b.side.take_edits())?;
        for root in ROOTS {
            same(step, "text", a.doc.text(root), b.doc.text(root))?;
        }
        same_state(
            step,
            "state",
            TWO,
            a.doc.encode_state().ok(),
            b.doc.encode_state().ok(),
        )?;
    }
    Ok(agent_edits)
}

#[test]
fn doc_paths_agree() {
    let (seeds, steps) = (
        env_u64("EDIT_PATHS_SEEDS", 24),
        env_u64("EDIT_PATHS_STEPS", 150) as usize,
    );
    let mut edits = 0;
    take_states();
    for seed in 0..seeds {
        match doc_sequence(seed, steps, CANDIDATE) {
            Ok(n) => edits += n,
            Err((step, why)) => panic!("seed {seed}, step {step}: {CANDIDATE:?} differs: {why}"),
        }
    }
    let (states, content_only) = take_states();
    eprintln!("[edit paths] doc level: {seeds} sequences of {steps} steps, {edits} agent edits; updates byte-identical; {states} states compared, {content_only} equal in content but not in bytes");
    let job = k2_mpc_base::differential::runrec_job();
    eprintln!(
        "RUNREC {{\"job\":{job},\"run\":\"edit_paths_doc\",\"tool\":\"edit_paths\",\"status\":\"COMPLETE\",\"candidate\":\"{CANDIDATE:?}\",\"sequences\":{seeds},\"steps\":{},\"agent_edits\":{edits},\"states_compared\":{states},\"states_content_only\":{content_only}}}",
        seeds * steps as u64
    );
    assert!(
        edits as u64 > seeds * steps as u64 / 3,
        "too few agent edits: {edits}"
    );
}

/// The doc-level driver catches both mutants, early.
#[test]
fn doc_driver_catches_wrong_paths() {
    for mutant in [Path::MutantFrontToBack, Path::MutantStale] {
        let mut caught = 0;
        for seed in 0..8 {
            if let Err((step, why)) = doc_sequence(seed, 150, mutant) {
                assert!(step < 150, "{why}");
                caught += 1;
            }
        }
        assert_eq!(caught, 8, "{mutant:?} got through the doc-level driver");
    }
}

// ---------- the session-level driver ----------

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    /// ann owns the doc (purge); everyone reads, edits and sees history.
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(who.pid == "ann")
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;
const WHO: [(&str, ParticipantKind); 5] = [
    ("ann", ParticipantKind::Person),
    ("bob", ParticipantKind::Person),
    ("bot1", ParticipantKind::Agent),
    ("bot2", ParticipantKind::Agent),
    ("bot3", ParticipantKind::Agent),
];

fn config() -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c.idle_leave_ms = u64::MAX / 4;
    c.compact_entries = 16;
    c
}

struct SessionSide {
    side: Side,
    dir: std::path::PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
    s: Arc<Session>,
    conns: Vec<Option<ConnId>>,
    /// Each participant's last read: (version, text), for `write --base`.
    reads: Vec<Option<(u64, String)>>,
}

impl Drop for SessionSide {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn file_of(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("files").join("notes.md")
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "notes.md".into(),
    }
}

impl SessionSide {
    fn open(dir: &std::path::Path, host: &Arc<TestHost>) -> (SessionManager, Arc<Session>) {
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
        let mgr = SessionManager::new(store, host.clone(), config());
        let s = mgr
            .open_file(Arc::new(MD), &key(), &file_of(dir), &dir.join("files"))
            .unwrap();
        (mgr, s)
    }

    fn new(tag: &str, seed: u64, path: Path) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-paths-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("files")).unwrap();
        let seed_text: String = (0..12).map(|i| format!("seed line {i}\n")).collect();
        std::fs::write(file_of(&dir), seed_text).unwrap();
        let host = Arc::new(TestHost {
            now: AtomicU64::new(1_000_000),
        });
        let mut side = Side::new(path).with_client_seed(seed).recording();
        let (mgr, s) = with_side(&mut side, || Self::open(&dir, &host));
        let mut me = Self {
            side,
            dir,
            host,
            mgr,
            s,
            conns: vec![None; WHO.len()],
            reads: vec![None; WHO.len()],
        };
        for i in 0..WHO.len() {
            me.join(i);
        }
        me
    }

    fn join(&mut self, i: usize) {
        let (pid, kind) = WHO[i];
        let p = Participant::new(pid, pid, "#336699", kind, "cli");
        let s = self.s.clone();
        let j = with_side(&mut self.side, || s.join(p, Stay::Held)).unwrap();
        self.conns[i] = Some(j.conn);
    }

    fn text(&self) -> String {
        self.s.read(self.any_conn()).unwrap().text_by_root[0]
            .1
            .clone()
    }

    fn any_conn(&self) -> ConnId {
        self.conns
            .iter()
            .flatten()
            .next()
            .copied()
            .expect("someone present")
    }

    fn restart(&mut self) {
        self.mgr.release().unwrap();
        let (dir, host) = (self.dir.clone(), self.host.clone());
        let (mgr, s) = with_side(&mut self.side, || Self::open(&dir, &host));
        self.mgr = mgr;
        self.s = s;
        self.conns = vec![None; WHO.len()];
        self.reads = vec![None; WHO.len()];
        for i in 0..WHO.len() {
            self.join(i);
        }
    }
}

#[derive(Debug, Clone)]
enum SessionOp {
    Edit(usize, Vec<Hunk>),
    Replace(usize, String),
    Read(usize),
    WriteBase(usize, Vec<Hunk>),
    Undo(usize, usize, UndoMode),
    Redo(usize, usize, UndoMode),
    Revert(usize, u64, bool),
    Restore(usize, usize),
    NameVersion(usize),
    PurgeChanges(usize, u64, u64),
    PurgeText(usize),
    PurgeParticipant(usize),
    Outside(Vec<Hunk>),
    Flush(usize),
    Tick(u64),
    Leave(usize),
    Restart,
}

fn session_op(rng: &mut SmallRng, s: &SessionSide) -> SessionOp {
    let who = rng.gen_range(0..WHO.len());
    let text = s.text();
    let version = s.s.status().unwrap().version;
    let mode = |rng: &mut SmallRng| match rng.gen_range(0..10) {
        0..=5 => UndoMode::All,
        6..=8 => UndoMode::Partial,
        _ => UndoMode::Drop,
    };
    let lo = |rng: &mut SmallRng| rng.gen_range(1..=version.max(1));
    match rng.gen_range(0..1000) {
        0..=299 => SessionOp::Edit(who, random_hunks(rng, &text)),
        300..=359 => {
            let mut t = text;
            if !t.is_empty() && rng.gen_bool(0.5) {
                let b = boundaries(&t);
                t.truncate(b[rng.gen_range(0..b.len())]);
            }
            t.push_str(&insert_text(rng));
            SessionOp::Replace(who, t)
        }
        360..=409 => SessionOp::Read(who),
        410..=479 => match &s.reads[who] {
            Some((_, base)) => SessionOp::WriteBase(who, random_hunks(rng, base)),
            None => SessionOp::Read(who),
        },
        480..=599 => SessionOp::Undo(who, rng.gen_range(1..3), mode(rng)),
        600..=679 => SessionOp::Redo(who, rng.gen_range(1..3), mode(rng)),
        680..=739 => SessionOp::Revert(who, lo(rng), rng.gen_bool(0.5)),
        740..=769 => SessionOp::Restore(who, rng.gen_range(0..64)),
        770..=789 => SessionOp::NameVersion(who),
        790..=804 => {
            let from = lo(rng);
            SessionOp::PurgeChanges(0, from, (from + rng.gen_range(0..3)).min(version.max(1)))
        }
        805..=812 => SessionOp::PurgeText(0),
        813..=815 => SessionOp::PurgeParticipant(rng.gen_range(2..WHO.len())),
        816..=879 => SessionOp::Outside(random_hunks(rng, &text)),
        880..=914 => SessionOp::Flush(who),
        915..=954 => SessionOp::Tick(rng.gen_range(1..3_000)),
        955..=989 => SessionOp::Leave(who),
        _ => SessionOp::Restart,
    }
}

/// Run one op on one side; its outcome, as text (outcomes hold no connection IDs).
fn run_session_op(s: &mut SessionSide, op: &SessionOp, step: usize) -> String {
    let sess = s.s.clone();
    let conn = |s: &SessionSide, i: usize| s.conns[i];
    s.host.now.fetch_add(7, Ordering::SeqCst);
    // someone absent rejoins first (as a call of its own, same on both sides)
    let who = match op {
        SessionOp::Edit(i, _)
        | SessionOp::Replace(i, _)
        | SessionOp::Read(i)
        | SessionOp::WriteBase(i, _)
        | SessionOp::Undo(i, ..)
        | SessionOp::Redo(i, ..)
        | SessionOp::Revert(i, ..)
        | SessionOp::Restore(i, _)
        | SessionOp::NameVersion(i)
        | SessionOp::PurgeChanges(i, ..)
        | SessionOp::PurgeText(i)
        | SessionOp::Flush(i) => Some(*i),
        SessionOp::PurgeParticipant(_) => Some(0),
        _ => None,
    };
    if let Some(i) = who {
        if conn(s, i).is_none() {
            s.join(i);
        }
    }
    let c = who.and_then(|i| conn(s, i));
    let now_text = s.text();
    let side = &mut s.side;
    let out = match op {
        SessionOp::Edit(_, hunks) => format!(
            "{:?}",
            with_side(side, || sess.edit(
                c.unwrap(),
                vec![Edit::Text {
                    root: ROOT.into(),
                    hunks: hunks.clone()
                }],
                None
            ))
        ),
        SessionOp::Replace(_, t) => format!(
            "{:?}",
            with_side(side, || sess.replace_text(c.unwrap(), ROOT, t))
        ),
        SessionOp::Read(i) => {
            let r = sess.read(c.unwrap()).unwrap();
            let t = r.text_by_root[0].1.clone();
            s.reads[*i] = Some((r.version, t.clone()));
            format!("read v{} {t:?}", r.version)
        }
        SessionOp::WriteBase(i, hunks) => {
            let (base, old) = s.reads[*i].clone().unwrap_or((0, String::new()));
            let new = k2_mpc_base::text::diff::apply(&old, hunks);
            format!(
                "{:?}",
                with_side(&mut s.side, || sess.write_base(
                    c.unwrap(),
                    ROOT,
                    base,
                    &new
                ))
            )
        }
        SessionOp::Undo(_, n, m) => format!(
            "{:?}",
            with_side(side, || sess
                .undo_with(c.unwrap(), &UndoRequest::steps(*n).mode(*m)))
        ),
        SessionOp::Redo(_, n, m) => format!(
            "{:?}",
            with_side(side, || sess
                .redo_with(c.unwrap(), &UndoRequest::steps(*n).mode(*m)))
        ),
        SessionOp::Revert(_, seq, partial) => format!(
            "{:?}",
            with_side(side, || sess.revert(c.unwrap(), *seq, *partial))
        ),
        SessionOp::Restore(_, k) => {
            let vs = sess.versions(c.unwrap()).unwrap();
            if vs.is_empty() {
                "no versions".into()
            } else {
                let vid = vs[k % vs.len()].id;
                format!("{:?}", with_side(side, || sess.restore(c.unwrap(), vid)))
            }
        }
        SessionOp::NameVersion(_) => format!(
            "{:?}",
            with_side(side, || sess.name_version(
                c.unwrap(),
                &format!("v-{step}"),
                None
            ))
        ),
        SessionOp::PurgeChanges(_, from, to) => format!(
            "{:?}",
            with_side(side, || sess.purge(
                c.unwrap(),
                PurgeRequest::new(PurgeScope::Changes {
                    from: *from,
                    to: *to
                })
            ))
        ),
        SessionOp::PurgeText(_) => {
            // a line of the text (or of history), 8 bytes or more
            let t = &now_text;
            let line = t.lines().find(|l| l.len() >= 8).unwrap_or("seed line 3");
            format!(
                "{:?}",
                with_side(&mut s.side, || sess.purge(
                    c.unwrap(),
                    PurgeRequest::new(PurgeScope::Text(line.to_owned())).and_remove()
                ))
            )
        }
        SessionOp::PurgeParticipant(i) => {
            let r = with_side(side, || {
                sess.purge(
                    c.unwrap(),
                    PurgeRequest::new(PurgeScope::Participant {
                        pid: WHO[*i].0.into(),
                        pseudonym: format!("erased-{step}"),
                    }),
                )
            });
            // their connections are gone
            s.conns[*i] = None;
            format!("{r:?}")
        }
        SessionOp::Outside(hunks) => {
            let new = k2_mpc_base::text::diff::apply(&now_text, hunks);
            std::fs::write(file_of(&s.dir), new).unwrap();
            format!("{:?}", with_side(side, || sess.check_disk()))
        }
        SessionOp::Flush(_) => format!("{:?}", with_side(side, || sess.flush(c.unwrap()))),
        SessionOp::Tick(ms) => {
            s.host.now.fetch_add(*ms, Ordering::SeqCst);
            let mgr = &s.mgr;
            format!("{:?}", with_side(side, || mgr.tick()))
        }
        // ann is the observer: she stays
        SessionOp::Leave(0) => "observer stays".into(),
        SessionOp::Leave(i) => match conn(s, *i) {
            Some(cn) => {
                s.conns[*i] = None;
                format!("{:?}", sess.leave(cn))
            }
            None => "absent".into(),
        },
        SessionOp::Restart => {
            s.restart();
            "restarted".into()
        }
    };
    let out = without_file_hashes(&out);
    // a tick may unload the session: open it again (the same on both sides)
    if matches!(op, SessionOp::Tick(_)) && s.s.status().is_err() {
        s.restart();
    }
    out
}

/// File hashes are keyed by each store's own random key: not the engine's, and not alike on
/// two sides. Every quoted run of 64 hex digits becomes `<hash>`.
fn without_file_hashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(k) = rest.find('"') {
        let tail = &rest[k + 1..];
        let hex = tail.bytes().take_while(u8::is_ascii_hexdigit).count();
        if hex == 64 && tail.as_bytes().get(64) == Some(&b'"') {
            out.push_str(&rest[..k]);
            out.push_str("<hash>");
            rest = &tail[65..];
        } else {
            out.push_str(&rest[..=k]);
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// Per kind of op: (calls, calls that returned `Ok`), and engine edits made.
type Tally = (std::collections::BTreeMap<String, (u64, u64)>, u64);

fn session_sequence(
    seed: u64,
    steps: usize,
    candidate: Path,
    tally: &mut Tally,
) -> Result<(), Diverged> {
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut a = SessionSide::new("a", seed, Path::Throwaway);
    let mut b = SessionSide::new("b", seed, candidate);
    same(0, "import edits", a.side.take_edits(), b.side.take_edits())?;
    for step in 0..steps {
        let op = session_op(&mut rng, &a);
        let ra = run_session_op(&mut a, &op, step);
        let rb = catch_unwind(AssertUnwindSafe(|| run_session_op(&mut b, &op, step)))
            .map_err(|_| (step, format!("the candidate panicked on {op:?}")))?;
        same(step, &format!("{op:?}: outcome"), &ra, &rb)?;
        let kind = format!("{op:?}");
        let kind = kind.split(['(', ' ']).next().unwrap_or("").to_owned();
        let t = tally.0.entry(kind).or_default();
        t.0 += 1;
        t.1 += u64::from(ra.starts_with("Ok("));
        let (ea, eb) = (a.side.take_edits(), b.side.take_edits());
        tally.1 += ea.len() as u64;
        same_edits(step, k2_mpc_base::session::Pack::schema(&MD), ea, eb)?;
        let (ca, cb) = (a.any_conn(), b.any_conn());
        same(step, "text", a.text(), b.text())?;
        same(
            step,
            "version",
            a.s.status().unwrap().version,
            b.s.status().unwrap().version,
        )?;
        same(
            step,
            "blame",
            format!("{:?}", a.s.blame(ca, ROOT, None)),
            format!("{:?}", b.s.blame(cb, ROOT, None)),
        )?;
        same(
            step,
            "log",
            format!("{:?}", a.s.log(ca, &LogQuery::default())),
            format!("{:?}", b.s.log(cb, &LogQuery::default())),
        )?;
    }
    Ok(())
}

#[test]
fn session_paths_agree() {
    let (seeds, steps) = (
        env_u64("EDIT_PATHS_SESSION_SEEDS", 4),
        env_u64("EDIT_PATHS_SESSION_STEPS", 150) as usize,
    );
    let mut tally = Tally::default();
    take_states();
    for seed in 0..seeds {
        if let Err((step, why)) = session_sequence(seed, steps, CANDIDATE, &mut tally) {
            panic!("seed {seed}, step {step}: {CANDIDATE:?} differs: {why}");
        }
    }
    let (states, content_only) = take_states();
    eprintln!(
        "[edit paths] session level: {seeds} sequences of {steps} steps, {} engine edits; \
         updates byte-identical; {states} states compared, {content_only} equal in content \
         but not in bytes; calls (ok/all): {}",
        tally.1,
        tally
            .0
            .iter()
            .map(|(k, (n, ok))| format!("{k} {ok}/{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let job = k2_mpc_base::differential::runrec_job();
    eprintln!(
        "RUNREC {{\"job\":{job},\"run\":\"edit_paths_session\",\"tool\":\"edit_paths\",\"status\":\"COMPLETE\",\"candidate\":\"{CANDIDATE:?}\",\"sequences\":{seeds},\"steps\":{},\"engine_edits\":{},\"states_compared\":{states},\"states_content_only\":{content_only}}}",
        seeds * steps as u64,
        tally.1
    );
    for kind in [
        "Edit",
        "WriteBase",
        "Undo",
        "Redo",
        "Revert",
        "Restore",
        "PurgeChanges",
        "Outside",
    ] {
        assert!(
            tally.0.get(kind).is_some_and(|t| t.1 > 0),
            "no {kind} call succeeded: the mix doesn't cover it"
        );
    }
}

/// The session-level driver catches both mutants.
#[test]
fn session_driver_catches_wrong_paths() {
    for mutant in [Path::MutantFrontToBack, Path::MutantStale] {
        let caught = (0..3)
            .filter(|seed| session_sequence(*seed, 150, mutant, &mut Tally::default()).is_err())
            .count();
        assert_eq!(caught, 3, "{mutant:?} got through the session-level driver");
    }
}

// ---------- the shadow mode ----------

/// Shadow mode: each edit is also made through the reference path on a twin, and any
/// difference panics. It agrees with itself over a doc-level sequence, and panics on a mutant.
#[test]
fn shadow_mode_checks_each_edit() {
    let run = |path: Path| {
        let mut side = Side::new(path).shadowed_by(Path::Throwaway);
        let mut rng = SmallRng::seed_from_u64(7);
        let mut doc = Doc::new(TWO).unwrap();
        for _ in 0..200 {
            let client = AGENTS[rng.gen_range(0..AGENTS.len())];
            let root = ROOTS[rng.gen_range(0..2)];
            let t = doc.text(root).unwrap();
            let hunks = random_hunks(&mut rng, &t);
            with_side(&mut side, || {
                doc.edit_as(
                    client,
                    &[Edit::Text {
                        root: root.into(),
                        hunks,
                    }],
                )
            })
            .unwrap();
        }
        side.shadow_checks()
    };
    assert_eq!(run(CANDIDATE), 200);
    for mutant in [Path::MutantFrontToBack, Path::MutantStale] {
        let r = catch_unwind(AssertUnwindSafe(|| run(mutant)));
        let msg = r
            .err()
            .and_then(|e| e.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert!(msg.contains("edit paths differ"), "{mutant:?}: {msg:?}");
    }
}
