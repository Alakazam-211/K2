//! The one-writer invariant (v0.9.3): **exactly one writer per client ID, ever.**
//!
//! The engine identifies content by `(client, clock)` and skips a block it already has, so
//! if two writers ever made changes as one client from the same clock, the second one's
//! content would be **dropped without an error** ([`a_second_writer_is_dropped_silently`]).
//! An edit as a client is safe only on a doc that holds everything that client ever wrote.
//!
//! In the kit, the writers are: each connection's client (minted fresh per join, recorded
//! in the store's client table, edits made on the live doc under the session lock); the
//! import client (recorded with the doc); the outside-edit client (one per doc, recorded;
//! since v0.9.5 its edits are made on the live doc, a three-way merge of the save); and the
//! disk replica's own clients (v0.9.5: an outside save the doc holds more than is made on
//! the replica as a fresh client the doc never holds, never recorded, replaced with the
//! replica at the next write-back). The checks, after
//! every call of a random workload ([`one_writer_per_client_under_a_random_workload`]):
//!
//! 1. every client with content in the live doc is recorded with an owner (so a fresh ID is
//!    never one of them: `fresh_client` skips recorded IDs);
//! 2. no two present connections share a client ID;
//! 3. the outside-edit client is the one recorded for it, and the disk replica, the one other
//!    doc edits are made on, is ahead of the live doc only for its own clients: ones the
//!    live doc has never held, never recorded, present or the outside-edit client (before
//!    v0.9.5 the outside-edit client's edits were made on the replica, which then had to
//!    keep up with the doc for it);
//! 4. every engine edit continued its client's history on the doc it was made on (its
//!    insertions start at that client's clock there) and the doc took all of it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use yrs::updates::decoder::Decode;

use super::*;
use crate::differential::{with_side, EditRecord, Path, Side};
use crate::doc::{ClientId, Doc};
use crate::schema::{Kind, Schema};
use crate::store::{DocKey, DocStore};

/// Who can write as which client ID, read from a session ([`Session::writers_for_tests`]).
#[derive(Debug, Clone, Default)]
pub(crate) struct Writers {
    /// The live doc's clocks, with each client's owner in the store's client table.
    pub live: Vec<(ClientId, u32, Option<String>)>,
    /// The disk replica's clocks, when the doc has a file.
    pub replica: Option<Vec<(ClientId, u32)>>,
    /// The outside-edit client the session holds, and the one the store records.
    pub external: Option<ClientId>,
    pub external_owned: Option<ClientId>,
    /// Present connections: (pid, client).
    pub members: Vec<(String, ClientId)>,
}

fn check(w: &Writers) -> Result<(), String> {
    for (c, k, owner) in &w.live {
        if *k > 0 && owner.is_none() {
            return Err(format!(
                "client {c} wrote {k} units but no owner is recorded"
            ));
        }
    }
    let mut seen: Vec<ClientId> = w.members.iter().map(|m| m.1).collect();
    seen.sort_unstable();
    if let Some(d) = seen.windows(2).find(|p| p[0] == p[1]) {
        return Err(format!("two connections write as client {}", d[0]));
    }
    if let Some(e) = w.external {
        if w.external_owned != Some(e) {
            return Err(format!(
                "the outside-edit client {e} isn't the recorded one ({:?})",
                w.external_owned
            ));
        }
        if w.members.iter().any(|m| m.1 == e) {
            return Err(format!(
                "a connection writes as the outside-edit client {e}"
            ));
        }
    }
    if let Some(replica) = &w.replica {
        let live = |c: ClientId| w.live.iter().find(|x| x.0 == c).map_or(0, |x| x.1);
        for &(c, k) in replica {
            let replica_only = live(c) == 0
                && !w.live.iter().any(|x| x.0 == c && x.2.is_some())
                && !w.members.iter().any(|m| m.1 == c)
                && w.external != Some(c)
                && w.external_owned != Some(c);
            if k > live(c) && !replica_only {
                return Err(format!("the disk replica is ahead of the live doc for {c}"));
            }
        }
    }
    Ok(())
}

/// Check 4: each edit's insertions are its client's only, start at its clock on the doc it
/// was made on, and the doc's clock after is their end (nothing was skipped as known).
fn check_edits(edits: &[EditRecord]) -> Result<(), String> {
    for e in edits {
        let Ok(u) = &e.update else { continue };
        let u = yrs::Update::decode_v1(u).map_err(|x| x.to_string())?;
        let mut mine = None;
        for (c, ranges) in u.insertions(false).iter() {
            if c.get() != e.client {
                return Err(format!("an edit as {} inserts as {}", e.client, c.get()));
            }
            let lo = ranges.iter().map(|r| r.start).min().unwrap_or(0);
            let hi = ranges.iter().map(|r| r.end).max().unwrap_or(0);
            mine = Some((lo, hi));
        }
        let after = e
            .state_vector
            .iter()
            .find(|x| x.0 == e.client)
            .map_or(0, |x| x.1);
        match mine {
            Some((lo, hi)) => {
                if lo != e.clock_before {
                    return Err(format!(
                        "an edit as {} starts at clock {lo}, the doc had {}",
                        e.client, e.clock_before
                    ));
                }
                if after != hi {
                    return Err(format!(
                        "an edit as {} ends at clock {hi}, the doc's is {after}: content dropped",
                        e.client
                    ));
                }
            }
            None if after != e.clock_before => {
                return Err(format!(
                    "a delete-only edit as {} moved its clock",
                    e.client
                ));
            }
            None => {}
        }
    }
    Ok(())
}

const S: Schema = Schema {
    name: "t",
    roots: &[("source", Kind::Text)],
};

/// Why the invariant matters: two docs at one state, each edited as client 9 from the same
/// clock. Applying one's update to the other is accepted (no error), and its text is gone.
#[test]
fn a_second_writer_is_dropped_silently() {
    let mut a = Doc::new(S).unwrap();
    a.replace_text_as(5, "source", "base\n").unwrap();
    let mut b = Doc::load(S, &a.encode_state().unwrap()).unwrap();
    a.replace_text_as(9, "source", "base\nfrom a\n").unwrap();
    let ub = b
        .replace_text_as(9, "source", "base\nfrom b\n")
        .unwrap()
        .unwrap();
    assert_eq!(a.clock_of(9), b.clock_of(9), "both wrote 9's clocks 0..7");
    a.apply_trusted(&ub).unwrap();
    assert_eq!(
        a.text("source").unwrap(),
        "base\nfrom a\n",
        "b's text is dropped"
    );
}

/// The checks fail when they should.
#[test]
fn the_checks_catch_a_second_writer() {
    let ok = Writers {
        live: vec![
            (7, 10, Some("ann".into())),
            (8, 3, Some(EXTERNAL_PID.into())),
        ],
        replica: Some(vec![(7, 4), (8, 3)]),
        external: Some(8),
        external_owned: Some(8),
        members: vec![("ann".into(), 7)],
    };
    check(&ok).unwrap();
    let mut w = ok.clone();
    w.live[0].2 = None;
    assert!(check(&w).unwrap_err().contains("no owner"));
    let mut w = ok.clone();
    w.members.push(("bob".into(), 7));
    assert!(check(&w).unwrap_err().contains("two connections"));
    // the replica's own client (an outside save the doc holds more than, v0.9.5)
    let mut w = ok.clone();
    w.replica = Some(vec![(7, 4), (8, 2), (11, 5)]);
    check(&w).unwrap();
    // ahead for a writer of the live doc's
    let mut w = ok.clone();
    w.replica = Some(vec![(7, 11), (8, 3)]);
    assert!(check(&w).unwrap_err().contains("ahead"));
    let mut w = ok.clone();
    w.replica = Some(vec![(7, 4), (8, 4)]);
    assert!(check(&w).unwrap_err().contains("ahead"));
    let mut w = ok.clone();
    w.members.push(("bob".into(), 8));
    assert!(check(&w).unwrap_err().contains("outside-edit client"));
    // and an edit that skipped clocks, or that the doc only partly took
    let mut d = Doc::new(S).unwrap();
    let u = d.replace_text_as(9, "source", "abc").unwrap().unwrap();
    let rec = |clock_before, after| EditRecord {
        client: 9,
        clock_before,
        update: Ok(u.clone()),
        state: None,
        state_vector: vec![(9, after)],
    };
    check_edits(&[rec(0, 3)]).unwrap();
    assert!(check_edits(&[rec(2, 3)]).unwrap_err().contains("starts at"));
    assert!(check_edits(&[rec(0, 2)]).unwrap_err().contains("dropped"));
}

// ---------- the workload ----------

struct H {
    now: AtomicU64,
}

impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(who.pid == "p0")
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

struct World {
    dir: PathBuf,
    host: Arc<H>,
    mgr: SessionManager,
    s: Arc<Session>,
    conns: Vec<(usize, ConnId)>,
    /// Whether the workload undoes, redoes and reverts (see [`undo_cross_check_repro`]).
    history: bool,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn file(dir: &std::path::Path) -> PathBuf {
    dir.join("ws").join("a.md")
}

fn open(dir: &std::path::Path, host: &Arc<H>) -> (SessionManager, Arc<Session>) {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let c = Config {
        edit_rate_per_s: 1_000_000,
        edit_burst: 1_000_000,
        compact_entries: 8,
        ..Config::default()
    };
    let mgr = SessionManager::new(store, host.clone(), c);
    let k = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let s = mgr
        .open_file(Arc::new(MD), &k, &file(dir), &dir.join("ws"))
        .unwrap();
    (mgr, s)
}

const PIDS: [(&str, ParticipantKind); 5] = [
    ("p0", ParticipantKind::Person),
    ("p1", ParticipantKind::Person),
    ("a0", ParticipantKind::Agent),
    ("a1", ParticipantKind::Agent),
    ("a2", ParticipantKind::Agent),
];

impl World {
    fn new(seed: u64, history: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("k2mb-1writer-{}-{seed}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        std::fs::write(file(&dir), "one\ntwo\nthree\n").unwrap();
        let host = Arc::new(H {
            now: AtomicU64::new(1_000_000),
        });
        let (mgr, s) = open(&dir, &host);
        Self {
            dir,
            host,
            mgr,
            s,
            conns: Vec::new(),
            history,
        }
    }

    fn join(&mut self, i: usize) {
        let (pid, kind) = PIDS[i];
        let p = Participant::new(pid, pid, "#123456", kind, "cli");
        let conn = self.s.join(p, Stay::Held).unwrap().conn;
        self.conns.push((i, conn));
    }

    fn text(&self) -> String {
        let c = self.conns[0].1;
        self.s.read(c).unwrap().text_by_root[0].1.clone()
    }

    fn step(&mut self, rng: &mut SmallRng, n: usize) {
        self.host.now.fetch_add(13, Ordering::SeqCst);
        if self.conns.is_empty() || rng.gen_bool(0.08) {
            self.join(rng.gen_range(0..PIDS.len()));
            return;
        }
        let (who, conn) = self.conns[rng.gen_range(0..self.conns.len())];
        let mut kind = rng.gen_range(0..100);
        if !self.history && (45..=72).contains(&kind) {
            kind = 0; // an edit instead
        }
        if std::env::var_os("ONE_WRITER_TRACE").is_some() {
            eprintln!("  {} kind {kind} text {:?}", PIDS[who].0, self.text());
        }
        match kind {
            0..=44 => {
                let t = self.text();
                let line = format!("{}-{n}\n", PIDS[who].0);
                let at = t
                    .match_indices('\n')
                    .map(|(k, _)| k + 1)
                    .nth(rng.gen_range(0..4));
                let new = match at {
                    Some(k) if rng.gen_bool(0.3) => {
                        format!("{}{}", &t[..k], t[k..].replacen('\n', "", 1))
                    }
                    Some(k) => format!("{}{line}{}", &t[..k], &t[k..]),
                    None => format!("{t}{line}"),
                };
                let _ = self
                    .s
                    .replace_text(conn, crate::session::TextFilePack::ROOT, &new);
            }
            45..=59 => {
                let _ = self
                    .s
                    .undo_with(conn, &UndoRequest::steps(1).mode(UndoMode::Partial));
            }
            60..=67 => {
                let _ = self.s.redo(conn, 1);
            }
            68..=72 => {
                let v = self.s.status().unwrap().version;
                let _ = self.s.revert(conn, rng.gen_range(1..=v.max(1)), true);
            }
            73..=82 => {
                // an outside edit to the file, merged in (the outside-edit client)
                let t = self.text();
                std::fs::write(file(&self.dir), format!("outside {n}\n{t}")).unwrap();
                let _ = self.s.check_disk();
            }
            83..=88 => {
                let _ = self.s.flush(conn);
            }
            89..=93 => {
                let k = self.conns.iter().position(|c| c.1 == conn).unwrap();
                let _ = self.s.leave(conn);
                self.conns.remove(k);
            }
            94..=96 => {
                if PIDS[who].0 == "p0" && rng.gen_bool(0.3) {
                    let victim = PIDS[rng.gen_range(2..PIDS.len())].0;
                    let _ = self.s.purge(
                        conn,
                        PurgeRequest::new(PurgeScope::Participant {
                            pid: victim.into(),
                            pseudonym: format!("gone-{n}"),
                        }),
                    );
                    self.conns.retain(|c| PIDS[c.0].0 != victim);
                }
            }
            _ => {
                // a restart: everything reloads from the store; everyone rejoins
                self.mgr.release().unwrap();
                let (mgr, s) = open(&self.dir, &self.host);
                self.mgr = mgr;
                self.s = s;
                let who: Vec<usize> = self.conns.drain(..).map(|c| c.0).collect();
                for i in who {
                    self.join(i);
                }
            }
        }
    }
}

/// The workload, `seeds` seeds of `calls` calls, with the checks after every call.
fn workload(seeds: std::ops::Range<u64>, calls: usize, history: bool) {
    let mut edits = 0;
    let mut outside = 0;
    for seed in seeds.clone() {
        let mut rng = SmallRng::seed_from_u64(seed);
        let mut side = Side::new(Path::MainDoc).with_client_seed(seed).recording();
        let mut w = with_side(&mut side, || World::new(seed, history));
        let mut externals = std::collections::HashSet::new();
        for n in 0..calls {
            if std::env::var_os("ONE_WRITER_TRACE").is_some() {
                eprintln!("[one writer] seed {seed}, call {n}");
            }
            if std::env::var("ONE_WRITER_DUMP")
                .ok()
                .and_then(|v| v.parse().ok())
                == Some(n)
            {
                let c = w.conns[0].1;
                let t = w.text();
                eprintln!("[dump] text {t:?}");
                let b =
                    w.s.blame(c, crate::session::TextFilePack::ROOT, None)
                        .unwrap();
                eprintln!("[dump] blame\n{}", b.porcelain());
            }
            with_side(&mut side, || w.step(&mut rng, n));
            let rec = side.take_edits();
            edits += rec.len();
            if let Err(e) = check_edits(&rec) {
                panic!("seed {seed}, call {n}: {e}");
            }
            let writers = w.s.writers_for_tests().unwrap();
            if let Err(e) = check(&writers) {
                panic!("seed {seed}, call {n}: {e}\n{writers:?}");
            }
            externals.extend(writers.external_owned);
        }
        assert!(
            externals.len() <= 1,
            "one outside-edit client per doc: {externals:?}"
        );
        outside += externals.len();
    }
    assert!(
        edits > 100 * (seeds.end - seeds.start) as usize,
        "too few edits: {edits}"
    );
    assert!(outside > 0, "no outside edit was merged");
}

/// Edits (people and agents), undo, redo and revert, outside edits merged in, flushes,
/// leaves and rejoins, purging a participant, restarts.
///
/// With undo in the mix this workload found (v0.9.3) that an outside edit was reported to the
/// undo regions as a diff of the live text, not at element identity: seed 1, call 129, a0's
/// `undo --partial` of its own line conflicted after an outside edit had put a copy of it just
/// before it (the undo merge's debug cross-check caught it; `tests/outside_edit_undo.rs`).
#[test]
fn one_writer_per_client_under_a_random_workload() {
    let seeds: u64 = std::env::var("ONE_WRITER_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    workload(0..seeds, 250, true);
}

/// Agent client IDs are reused (v0.9.7, `Config::reuse_agent_clients`), one writer at a time:
/// an agent's next connection takes a client ID it used before when no present connection
/// holds it (after a restart too: the store records them); two connections at once get two
/// IDs; a person's and an own-replica participant's IDs are never reused; with the setting
/// off, every join mints. Each reused ID's edits continue its history (check 4) and the
/// checks hold after every call.
#[test]
fn agent_client_ids_are_reused_one_writer_at_a_time() {
    let mut side = Side::new(Path::MainDoc).with_client_seed(9).recording();
    let mut w = with_side(&mut side, || World::new(90, false));
    let agent = |pid: &str| Participant::new(pid, pid, "#123456", ParticipantKind::Agent, "cli");
    let person = |pid: &str| Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab");
    let checked = |w: &World, side: &mut Side| {
        check_edits(&side.take_edits()).unwrap();
        check(&w.s.writers_for_tests().unwrap()).unwrap();
    };
    let edit = |w: &World, side: &mut Side, conn: ConnId, n: usize| {
        with_side(side, || {
            let t = w.s.read(conn).unwrap().text_by_root[0].1.clone();
            w.s.replace_text(conn, TextFilePack::ROOT, &format!("{t}line {n}\n"))
                .unwrap();
        });
    };
    let s = w.s.clone();
    let join = |side: &mut Side, s: &Arc<Session>, p: Participant| {
        with_side(side, || s.join(p, Stay::Held).unwrap())
    };
    // one agent, one connection after another: the same ID, its edits continuing
    let j1 = join(&mut side, &s, agent("a0"));
    edit(&w, &mut side, j1.conn, 1);
    checked(&w, &mut side);
    s.leave(j1.conn).unwrap();
    let j2 = join(&mut side, &s, agent("a0"));
    assert_eq!(
        j2.client, j1.client,
        "the agent's next connection reuses its ID"
    );
    edit(&w, &mut side, j2.conn, 2);
    checked(&w, &mut side);
    // two at once: two IDs
    let j3 = join(&mut side, &s, agent("a0"));
    assert_ne!(
        j3.client, j2.client,
        "a second connection at once gets its own ID"
    );
    edit(&w, &mut side, j3.conn, 3);
    checked(&w, &mut side);
    s.leave(j2.conn).unwrap();
    s.leave(j3.conn).unwrap();
    // a person: a fresh ID every time
    let p1 = join(&mut side, &s, person("p1"));
    s.leave(p1.conn).unwrap();
    let p2 = join(&mut side, &s, person("p1"));
    assert_ne!(p1.client, p2.client, "a person's ID is never reused");
    s.leave(p2.conn).unwrap();
    // an agent with its own replica: never reused, even by the same agent later
    let r1 = join(&mut side, &s, agent("a1").own_replica());
    s.leave(r1.conn).unwrap();
    let r2 = join(&mut side, &s, agent("a1"));
    assert_ne!(r2.client, r1.client, "an own-replica ID is never reused");
    s.leave(r2.conn).unwrap();
    // after a restart the store still names the agent's IDs: the lowest free one is taken
    w.mgr.release().unwrap();
    let (mgr, s2) = with_side(&mut side, || open(&w.dir, &w.host));
    w.mgr = mgr;
    w.s = s2.clone();
    let j4 = join(&mut side, &s2, agent("a0"));
    assert_eq!(
        j4.client,
        j1.client.min(j3.client),
        "reused after a restart"
    );
    edit(&w, &mut side, j4.conn, 4);
    checked(&w, &mut side);
    s2.leave(j4.conn).unwrap();
}

/// With `Config::reuse_agent_clients` off, every join mints a client ID, as before v0.9.7.
#[test]
fn with_reuse_off_every_join_mints() {
    let dir = std::env::temp_dir().join(format!("k2mb-reuse-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
    });
    let c = Config {
        reuse_agent_clients: false,
        ..Config::default()
    };
    let mgr = SessionManager::new(store, host, c);
    let k = DocKey {
        space: "ws".into(),
        path: "b.md".into(),
    };
    let s = mgr.open(Arc::new(MD), &k, Some(b"x\n")).unwrap();
    let a = || Participant::new("a0", "a0", "#123456", ParticipantKind::Agent, "cli");
    let j1 = s.join(a(), Stay::Held).unwrap();
    s.leave(j1.conn).unwrap();
    let j2 = s.join(a(), Stay::Held).unwrap();
    assert_ne!(j1.client, j2.client);
    drop(mgr);
    let _ = std::fs::remove_dir_all(&dir);
}
