//! Agent client-ID reuse (`Config::reuse_agent_clients`) across a reload of the doc's session.
//!
//! An agent that joined with its own replica (`Participant::own_replica`) keeps a replica that
//! writes as its client ID; that ID must never be handed to another connection, or two writers
//! share one ID and the engine drops one's content silently. The store records what each
//! agent's or own replica's ID was minted for with its owner (`client_kinds`), so after the
//! session unloads (a host's close, an idle unload, a store reopen) an own replica's ID is
//! still never reused, while a plain agent's still is. An owner record of unknown kind (a kit
//! before `client_kinds` wrote it) is never reused.
//!
//! Fail-first: on 8b9b1418 the own-replica reload tests fail at their assertions (reuse took
//! any earlier ID of the pid, an own replica's included, once the session that knew it had
//! unloaded).
//!
//! Planted faults (`MPC_STRUCT_MUTANT`, the setting echoed): `reuse-across-reload`
//! (`ClientMutant::ReuseAcrossReload`: the recorded kinds ignored) fails
//! the own-replica and N−1 tests; `no-reuse-after-reload` (`ClientMutant::NoReuseAfterReload`:
//! no ID from before the load reused) fails the plain-agent test; `no-agent-row-on-gain`
//! (`ClientMutant::NoAgentRowOnGain`: no kind row when an agent gains edit rights after a
//! read-only join) fails `an_agent_gaining_edit_after_a_read_only_join_reuses_its_id` at its
//! row assert, as `soak_agent_runs_ids_bounded` does at its read-only arm's.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, Rights, Session, SessionManager, Stay,
    TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};

fn under_mutant<R>(f: impl FnOnce() -> R) -> R {
    let m = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("reuse-across-reload") => {
            use k2_mpc_base::differential::{with_client_mutant, ClientMutant};
            with_client_mutant(ClientMutant::ReuseAcrossReload, f)
        }
        Ok("no-reuse-after-reload") => {
            use k2_mpc_base::differential::{with_client_mutant, ClientMutant};
            with_client_mutant(ClientMutant::NoReuseAfterReload, f)
        }
        Ok("no-agent-row-on-gain") => {
            use k2_mpc_base::differential::{with_client_mutant, ClientMutant};
            with_client_mutant(ClientMutant::NoAgentRowOnGain, f)
        }
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this suite plants"),
        Err(_) => f(),
    }
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Dir(std::path::PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(name: &str) -> Dir {
    let d = std::env::temp_dir().join(format!("k2mpc-reuse-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Dir(d)
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    }
}

fn manager(d: &Dir) -> SessionManager {
    manager_on(Arc::new(DocStore::open(d.0.join("d.redb")).unwrap()))
}

fn manager_on(store: Arc<DocStore>) -> SessionManager {
    SessionManager::new(store, Arc::new(H(AtomicU64::new(1))), Config::default())
}

/// A host that grants edit only while its gate is open (as a host that grants edit after an
/// approval, inside the connection).
struct Gate(AtomicBool, AtomicU64);
impl Host for Gate {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        if self.0.load(Ordering::SeqCst) {
            Rights::full()
        } else {
            Rights::read_only()
        }
    }
    fn now_ms(&self) -> u64 {
        self.1.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn agent(pid: &str) -> Participant {
    Participant::new(pid, pid, "#000000", ParticipantKind::Agent, "cli")
}

fn join(s: &Session, p: Participant) -> (k2_mpc_base::session::ConnId, u64) {
    let j = s.join(p, Stay::UntilIdle).unwrap();
    (j.conn, j.client)
}

/// The shipped hazard: an agent's own replica writes as client C; the session unloads (here a
/// host's close) and loads again; the same agent joins again without a replica. Its new
/// connection must not get C, which the old replica may still write as.
#[test]
fn an_own_replicas_id_is_never_reused_after_a_close_and_reopen() {
    under_mutant(|| {
        let d = dir("close");
        let mgr = manager(&d);
        let s = mgr.open(Arc::new(MD), &key(), Some(b"x\n")).unwrap();
        let (conn, replica) = join(&s, agent("bot").own_replica());
        assert!(s.leave(conn).unwrap());
        assert!(mgr.close(&key(), "test").unwrap());
        drop(s);
        let s = mgr.open(Arc::new(MD), &key(), None).unwrap();
        let (_, again) = join(&s, agent("bot"));
        assert_ne!(
            again, replica,
            "after a reload the agent got its own replica's client ID back"
        );
    });
}

/// The same across a store reopen (a new process).
#[test]
fn an_own_replicas_id_is_never_reused_after_a_store_reopen() {
    under_mutant(|| {
        let d = dir("reopen");
        let replica = {
            let mgr = manager(&d);
            let s = mgr.open(Arc::new(MD), &key(), Some(b"x\n")).unwrap();
            let (conn, c) = join(&s, agent("bot").own_replica());
            assert!(s.leave(conn).unwrap());
            mgr.release().unwrap();
            c
        };
        let mgr = manager(&d);
        let s = mgr.open(Arc::new(MD), &key(), None).unwrap();
        let (_, again) = join(&s, agent("bot"));
        assert_ne!(
            again, replica,
            "after a store reopen the agent got its own replica's client ID back"
        );
    });
}

/// Reuse survives a reload for a plain agent (no replica): it leaves and comes back and gets
/// its ID again, within the load, after a host's close and reopen, and after a store reopen.
/// An ID per agent per load would grow the doc's state with every idle unload.
#[test]
fn a_plain_agents_id_is_reused_after_a_reload() {
    under_mutant(|| {
        let d = dir("plain");
        let first = {
            let mgr = manager(&d);
            let s = mgr.open(Arc::new(MD), &key(), Some(b"x\n")).unwrap();
            let (conn, first) = join(&s, agent("bot"));
            assert!(s.leave(conn).unwrap());
            let (conn, second) = join(&s, agent("bot"));
            assert_eq!(second, first, "reused within the load");
            assert!(s.leave(conn).unwrap());
            assert!(mgr.close(&key(), "test").unwrap());
            drop(s);
            let s = mgr.open(Arc::new(MD), &key(), None).unwrap();
            let (conn, third) = join(&s, agent("bot"));
            assert_eq!(third, first, "reused after a close and reopen");
            assert!(s.leave(conn).unwrap());
            mgr.release().unwrap();
            first
        };
        let mgr = manager(&d);
        let s = mgr.open(Arc::new(MD), &key(), None).unwrap();
        let (_, fourth) = join(&s, agent("bot"));
        assert_eq!(fourth, first, "reused after a store reopen");
    });
}

/// N−1: a store whose owner records a kit before `client_kinds` wrote (an agent's, and one
/// its own replica may still write as: the record can't tell) has no kinds. None of those IDs
/// is reused after a reload; the agent draws a fresh ID, recorded with its kind, and that one
/// is reused from then on, across reloads. An older kit that writes owner records between
/// this kit's loads (here, an own replica's) is held to the same rule.
#[test]
fn an_older_kits_client_ids_are_never_reused() {
    under_mutant(|| {
        let d = dir("older");
        let store = Arc::new(DocStore::open(d.0.join("d.redb")).unwrap());
        let reopen = |mgr: &SessionManager, s: Arc<Session>| {
            assert!(mgr.close(&key(), "test").unwrap());
            drop(s);
            mgr.open(Arc::new(MD), &key(), None).unwrap()
        };
        let mgr = manager_on(store.clone());
        let s = mgr.open(Arc::new(MD), &key(), Some(b"x\n")).unwrap();
        assert!(mgr.close(&key(), "test").unwrap());
        drop(s);
        // what an older kit's joins left: owner records only (the lowest IDs there are, so a
        // reuse that took them would take them first)
        let id = store.find(&key()).unwrap().expect("the doc");
        store.set_client(id, 2, "bot").unwrap();
        store.set_client(id, 3, "bot").unwrap();
        let older = [2, 3];
        let s = mgr.open(Arc::new(MD), &key(), None).unwrap();
        let (conn, fresh) = join(&s, agent("bot"));
        assert!(
            !older.contains(&fresh),
            "an older kit's client ID {fresh} was reused"
        );
        assert!(s.leave(conn).unwrap());
        let (conn, again) = join(&s, agent("bot"));
        assert_eq!(again, fresh, "the fresh ID is reused within the load");
        assert!(s.leave(conn).unwrap());
        let s = reopen(&mgr, s);
        let (conn, after) = join(&s, agent("bot"));
        assert_eq!(after, fresh, "the fresh ID is reused after a reload");
        assert!(s.leave(conn).unwrap());
        // an older kit between this kit's loads: an own replica's owner record, no kind
        assert!(mgr.close(&key(), "test").unwrap());
        drop(s);
        store.set_client(id, 4, "bot").unwrap();
        let older = [2, 3, 4];
        let s = mgr.open(Arc::new(MD), &key(), None).unwrap();
        let (_, held) = join(&s, agent("bot"));
        assert_eq!(held, fresh, "the marked ID is still the one reused");
        // the marked ID is held: a second connection mints, never an older kit's ID
        let (_, second) = join(&s, agent("bot"));
        assert!(
            !older.contains(&second) && second != fresh,
            "a second connection got client ID {second} (older: {older:?}, held: {fresh})"
        );
    });
}

/// An agent whose every join is read-only, and which gains edit rights inside each connection
/// (a host that grants edit after an approval), takes one client ID and reuses it, within the
/// load and after a reload: its ID's kind is recorded when it first gains edit rights, not only
/// at a join that has them.
#[test]
fn an_agent_gaining_edit_after_a_read_only_join_reuses_its_id() {
    under_mutant(|| {
        let d = dir("gain");
        let gate = Arc::new(Gate(AtomicBool::new(false), AtomicU64::new(1)));
        let store = Arc::new(DocStore::open(d.0.join("d.redb")).unwrap());
        let mgr = SessionManager::new(store, gate.clone(), Config::default());
        let s = mgr.open(Arc::new(MD), &key(), Some(b"x\n")).unwrap();
        let mut ids = std::collections::BTreeSet::new();
        for k in 0..5 {
            gate.0.store(false, Ordering::SeqCst);
            let j = s.join(agent("bot"), Stay::UntilIdle).unwrap();
            gate.0.store(true, Ordering::SeqCst);
            let t = s.read(j.conn).unwrap().text_by_root.remove(0).1;
            s.replace_text(j.conn, TextFilePack::ROOT, &format!("{t}a{k}\n"))
                .unwrap();
            ids.insert(j.client);
            assert!(s.leave(j.conn).unwrap());
        }
        println!("MEASURE agent_gain ids={}", ids.len());
        assert_eq!(
            ids.len(),
            1,
            "row: an agent that gains edit after a read-only join reuses its ID: {ids:?}"
        );
        assert!(mgr.close(&key(), "test").unwrap());
        drop(s);
        gate.0.store(false, Ordering::SeqCst);
        let s = mgr.open(Arc::new(MD), &key(), None).unwrap();
        let (_, again) = join(&s, agent("bot"));
        assert!(
            ids.contains(&again),
            "row: the agent's ID is reused after a reload too"
        );
    });
}

/// The agent soak: `SOAK` runs of an agent, each a join, one edit and a leave. The doc's
/// writers: (a) one pid, edit rights at the join: one; (b) one pid, a read-only join granted
/// edit inside the connection: one; (c) a new pid per run (a host that stamps a run id as the
/// pid): one per run, which is the host's to change (a stable pid per agent).
#[test]
fn soak_agent_runs_ids_bounded() {
    const SOAK: usize = 200;
    under_mutant(|| {
        let run = |name: &str, gated: bool, pid_per_run: bool| {
            let d = dir(name);
            let gate = Arc::new(Gate(AtomicBool::new(true), AtomicU64::new(1)));
            let store = Arc::new(DocStore::open(d.0.join("d.redb")).unwrap());
            let mut c = Config::default();
            c.edit_rate_per_s = 1 << 30;
            c.edit_burst = 1 << 30;
            let mgr = SessionManager::new(store.clone(), gate.clone(), c);
            let s = mgr.open(Arc::new(MD), &key(), Some(b"x\n")).unwrap();
            for k in 0..SOAK {
                gate.0.store(!gated, Ordering::SeqCst);
                let pid = if pid_per_run {
                    format!("run-{k}")
                } else {
                    "bot".to_owned()
                };
                let j = s.join(agent(&pid), Stay::UntilIdle).unwrap();
                gate.0.store(true, Ordering::SeqCst);
                let t = s.read(j.conn).unwrap().text_by_root.remove(0).1;
                s.replace_text(j.conn, TextFilePack::ROOT, &format!("{t}{k}\n"))
                    .unwrap();
                assert!(s.leave(j.conn).unwrap());
            }
            let id = store.find(&key()).unwrap().expect("the doc");
            let writers: std::collections::BTreeSet<_> = store
                .load(id)
                .unwrap()
                .updates
                .iter()
                .map(|(_, e)| e.client)
                .collect();
            writers.len() - 1 // the import's
        };
        let a = run("soak-a", false, false);
        let b = run("soak-b", true, false);
        let c = run("soak-c", false, true);
        println!("MEASURE agent_soak runs={SOAK} writers_edit_at_join={a} writers_gain={b} writers_pid_per_run={c}");
        assert_eq!(
            a, 1,
            "row: an agent with edit rights at its joins keeps one writer"
        );
        assert_eq!(
            b, 1,
            "row: an agent granted edit inside each connection keeps one writer"
        );
        assert_eq!(c, SOAK, "row: a new pid per run takes a writer per run");
    });
}
