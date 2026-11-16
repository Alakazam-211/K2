//! Agents count toward the participant cap exactly as persons do. `Config::participants_max`
//! (32 by default) bounds the distinct pids that may write in a live session at once; past it a
//! new pid joins as a watcher (read-only, no presence detail, counted only as `watching`),
//! whatever its kind. These rows show an agent can't slip past it:
//!
//! - `agents_fill_the_cap_as_persons_do`: N persons and M agents with N + M at the cap fill
//!   it; the next agent (a new pid) joins read-only, and its edit is refused with the same
//!   error, id and exit a person past the cap gets (`MPC-E305`, exit 3);
//! - `own_replica_and_assistant_agents_take_a_slot_each`: an own-replica agent and an
//!   assistant acting for a person take a slot each, as any agent does;
//! - `an_agent_rejoining_under_its_own_pid_takes_no_second_slot`: a second connection, the
//!   shared one-shot connection, and a return after leaving (when `Config::reuse_agent_clients`
//!   hands its client ID back) take no second slot, and a reused client ID brings no slot;
//! - `a_cli_tool_connection_counts_as_an_agent`: a tool connection on the CLI path
//!   (`Session::join_shared` with `via: "cli"`, then `Session::run_tool`) can take the last
//!   slot; past the cap its edit verbs are refused as a person's, and its read verbs run.
//!
//! Public API only, on `Config::default()`. The persons-only rows are in `overflow.rs`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::session::{
    At, AuditEvent, Config, ConnId, Host, LineCol, Participant, ParticipantKind, Rights, Session,
    SessionError, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{session_exit, ToolOutput, ToolRunError};

const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "one\ntwo\nthree\n";
/// The default cap: the bound these rows check.
const CAP: usize = 32;

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct TmpDir(std::path::PathBuf);

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _dir: TmpDir,
    _mgr: SessionManager,
    s: Arc<Session>,
}

/// A doc on the default config (participants_max 32, agent client reuse on).
fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-agents-cap-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
    });
    let cfg = Config::default();
    assert_eq!(cfg.participants_max, CAP, "the default cap");
    assert!(cfg.reuse_agent_clients, "agent reuse is on by default");
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host, cfg);
    let key = DocKey {
        space: "ws".into(),
        path: "notes.md".into(),
    };
    let pack = Arc::new(TextFilePack::new("markdown", 4 << 20));
    let s = mgr.open(pack, &key, Some(DOC.as_bytes())).unwrap();
    Fx {
        _dir: TmpDir(dir),
        _mgr: mgr,
        s,
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab")
}

fn agent(pid: &str) -> Participant {
    Participant::new(pid, pid, "#7c3aed", ParticipantKind::Agent, "socket")
}

/// An agent on the CLI path: one-shot verbs through `join_shared`.
fn cli(pid: &str) -> Participant {
    Participant::new(pid, pid, "#0f766e", ParticipantKind::Agent, "cli")
}

/// The refusal past the cap.
fn past_cap() -> SessionError {
    SessionError::Watching { max: CAP }
}

fn lc(line: usize, col: usize) -> At {
    At::Line(LineCol { line, col })
}

impl Fx {
    fn join(&self, p: Participant) -> ConnId {
        self.s.join(p, Stay::Held).unwrap().conn
    }
    /// The CLI's connection for a one-shot verb.
    fn join_cli(&self, pid: &str) -> ConnId {
        let j = self.s.join_shared(cli(pid), Stay::UntilIdle);
        j.unwrap().conn
    }
    /// A tool verb through `conn`.
    fn tool(&self, conn: ConnId, verb: &str, stdin: &[u8]) -> Result<ToolOutput, ToolRunError> {
        let argv = [verb.to_owned()];
        self.s.run_tool(conn, ROOT, &argv, stdin)
    }
    /// The pids with a roster row: the participants, never a watcher.
    fn pids(&self, viewer: ConnId) -> Vec<String> {
        self.s
            .roster(viewer)
            .unwrap()
            .into_iter()
            .map(|e| e.participant.pid)
            .collect()
    }
    fn has_row(&self, viewer: ConnId, pid: &str) -> bool {
        self.pids(viewer).iter().any(|p| p == pid)
    }
    fn watching(&self, viewer: ConnId) -> usize {
        self.s.watching(viewer).unwrap()
    }
    fn edit(&self, c: ConnId) -> Result<(), SessionError> {
        let t = self.s.read(c)?.text_by_root.remove(0).1;
        self.s.replace_text(c, ROOT, &format!("{t}x\n")).map(|_| ())
    }
    fn version(&self) -> u64 {
        self.s.status().unwrap().version
    }
}

/// N persons + M agents = the cap, for several splits (agents alone among them): every one of
/// them is a participant and edits; the next agent, a new pid, watches, and its edit is refused
/// exactly as the next person's is (the same error, `MPC-E305`, exit 3), nothing applied.
#[test]
fn agents_fill_the_cap_as_persons_do() {
    for agents in [1, 2, CAP / 2, CAP - 1, CAP] {
        let persons = CAP - agents;
        let split = format!("{persons} persons + {agents} agents");
        let f = fx(&format!("mix{agents}"));
        let mut conns = Vec::new();
        for n in 0..persons {
            conns.push(f.join(person(&format!("p{n}"))));
        }
        for n in 0..agents {
            conns.push(f.join(agent(&format!("a{n}"))));
        }
        let first = conns[0];
        assert_eq!(f.pids(first).len(), CAP, "{split}: all participants");
        assert_eq!(f.watching(first), 0, "{split}: nobody watches");
        for c in &conns {
            f.edit(*c).expect("a participant edits");
        }

        // the cap is full: the next agent watches
        let late = f.join(agent("late-agent"));
        assert_eq!(f.watching(first), 1, "{split}: the agent watches");
        assert_eq!(f.pids(first).len(), CAP, "{split}: no slot taken");
        assert!(!f.has_row(first, "late-agent"), "{split}: a row");
        f.s.read(late).expect("a watching agent reads");
        let v = f.version();
        let by_agent = f.edit(late).expect_err("an agent past the cap edited");
        assert_eq!(f.version(), v, "{split}: the refused edit applied");

        // the same refusal a person past the cap gets
        let late_person = f.join(person("late-person"));
        assert_eq!(f.watching(first), 2);
        let by_person = f.edit(late_person).expect_err("a person edited");
        assert_eq!(by_agent, past_cap(), "{split}");
        assert_eq!(by_agent, by_person, "{split}: refused alike");
        assert_eq!(by_agent.report().id(), "MPC-E305");
        assert_eq!(by_agent.report().id(), by_person.report().id());
        assert_eq!(session_exit(&by_agent), 3);
        assert_eq!(session_exit(&by_agent), session_exit(&by_person));
        // nor a caret, as for a watching person
        let caret = f.s.set_cursor(late, ROOT, lc(1, 1), lc(1, 1));
        assert_eq!(caret.map(|_| ()), Err(past_cap()), "{split}: a caret");
        // the participants still edit
        f.edit(first).unwrap();
    }
}

/// The agent kinds a host stamps: an own-replica agent and an assistant acting for a person
/// take a slot each, as a plain agent does; with them the cap is full and the next agent of
/// either kind watches.
#[test]
fn own_replica_and_assistant_agents_take_a_slot_each() {
    let f = fx("kinds");
    let first = f.join(person("p0"));
    for n in 1..CAP - 2 {
        f.join(person(&format!("p{n}")));
    }
    let replica = f.join(agent("replica").own_replica());
    let helper = f.join(agent("helper").on_behalf_of("p0"));
    assert_eq!(f.pids(first).len(), CAP);
    assert_eq!(f.watching(first), 0);
    f.edit(replica).unwrap();
    f.edit(helper).unwrap();
    let late = f.join(agent("late-replica").own_replica());
    assert_eq!(f.watching(first), 1);
    assert_eq!(f.edit(late), Err(past_cap()));
    let late_helper = f.join(agent("late-helper").on_behalf_of("p0"));
    assert_eq!(f.watching(first), 2);
    assert_eq!(f.edit(late_helper), Err(past_cap()));
}

/// An agent joining again under its own pid takes no second slot: a second connection while
/// its first is open, its shared (one-shot) connection twice, and, after it left, a join that
/// gets its client ID back (`Config::reuse_agent_clients`) and its one slot. A reused client ID
/// brings no slot: when the freed slot was taken meanwhile, the returning agent watches.
#[test]
fn an_agent_rejoining_under_its_own_pid_takes_no_second_slot() {
    let f = fx("rejoin");
    let first = f.join(person("p0"));
    for n in 1..CAP / 2 {
        f.join(person(&format!("p{n}")));
    }
    for n in 1..CAP / 2 {
        f.join(agent(&format!("a{n}")));
    }
    let j0 = f.s.join(agent("bot"), Stay::Held).unwrap();
    f.edit(j0.conn).unwrap();
    assert_eq!(f.pids(first).len(), CAP, "the cap is full");
    assert_eq!(f.watching(first), 0);

    // a second connection while the first is open
    let j1 = f.s.join(agent("bot"), Stay::Held).unwrap();
    f.edit(j1.conn).expect("a second connection edits");
    // the shared one-shot connection, twice: one connection
    let s1 = f.s.join_shared(agent("bot"), Stay::UntilIdle).unwrap();
    let s2 = f.s.join_shared(agent("bot"), Stay::UntilIdle).unwrap();
    assert_eq!(s1.conn, s2.conn, "the shared connection is reused");
    f.edit(s1.conn).unwrap();
    assert_eq!(f.pids(first).len(), CAP, "one pid, one slot");
    assert_eq!(f.watching(first), 0, "a pid already here watches");

    // it leaves altogether and comes back: a client ID of its own, and its one slot
    let mine = [j0.client, j1.client, s1.client];
    for c in [j0.conn, j1.conn, s1.conn] {
        assert!(f.s.leave(c).unwrap());
    }
    assert_eq!(f.pids(first).len(), CAP - 1);
    let back = f.s.join(agent("bot"), Stay::Held).unwrap();
    assert!(mine.contains(&back.client), "reuse is on: {mine:?}");
    f.edit(back.conn).unwrap();
    assert_eq!(f.pids(first).len(), CAP);
    assert_eq!(f.watching(first), 0);
    let again = f.s.join(agent("bot"), Stay::Held).unwrap();
    f.edit(again.conn).unwrap();
    assert_eq!(f.pids(first).len(), CAP, "still one slot");

    // the cap holds: a new agent watches
    let late = f.join(agent("late-agent"));
    assert_eq!(f.watching(first), 1);
    assert_eq!(f.edit(late), Err(past_cap()));
    // a watching agent joining again under its own pid stays one watcher
    let late2 = f.join(agent("late-agent"));
    assert_eq!(f.edit(late2), Err(past_cap()));
    assert_eq!(f.watching(first), 1);

    // a reused client ID brings no slot: the agent leaves, a new agent takes the slot, and
    // the agent's return (its client ID back) watches
    for c in [back.conn, again.conn] {
        assert!(f.s.leave(c).unwrap());
    }
    let taker = f.join(agent("taker"));
    f.edit(taker).expect("the freed slot is the new agent's");
    assert_eq!(f.pids(first).len(), CAP);
    let ret = f.s.join(agent("bot"), Stay::Held).unwrap();
    assert!(mine.contains(&ret.client), "its client ID back");
    assert_eq!(f.watching(first), 2, "the returning agent watches");
    assert!(!f.has_row(first, "bot"));
    assert_eq!(f.edit(ret.conn), Err(past_cap()));
}

/// A tool connection on the CLI path (`join_shared` + `run_tool`) counts as an agent: it takes
/// the last slot, its next verbs share its one connection, and a person after it watches. A new
/// CLI tool past the cap reads, and its edit verb is refused as the person's past the cap is
/// (the same `ToolRunError`, `MPC-E305`, exit 3), nothing applied.
#[test]
fn a_cli_tool_connection_counts_as_an_agent() {
    let f = fx("cli");
    let first = f.join(person("p0"));
    for n in 1..CAP - 1 {
        f.join(person(&format!("p{n}")));
    }
    assert_eq!(f.pids(first).len(), CAP - 1);

    let t = f.join_cli("tool");
    let out = f.tool(t, "append", b"tool\n").unwrap();
    assert_eq!(out.exit, 0, "{}", out.stderr_summary);
    assert!(out.changed.is_some(), "the tool's edit applied");
    assert_eq!(f.join_cli("tool"), t, "the next verb shares it");
    assert_eq!(f.tool(t, "cat", b"").unwrap().exit, 0);
    assert_eq!(f.pids(first).len(), CAP, "the tool took the last slot");
    assert!(f.has_row(first, "tool"));
    assert_eq!(f.watching(first), 0);

    // full: a new person watches
    let late_person = f.join(person("late-person"));
    assert_eq!(f.watching(first), 1, "the tool's slot counted");
    assert_eq!(f.edit(late_person), Err(past_cap()));

    // a new CLI tool past the cap: its read verb runs, its edit verb is refused
    let late_tool = f.join_cli("late-tool");
    assert_eq!(f.watching(first), 2, "the late tool watches");
    assert!(!f.has_row(first, "late-tool"));
    assert_eq!(f.tool(late_tool, "cat", b"").unwrap().exit, 0);
    let v = f.version();
    let by_tool = f.tool(late_tool, "append", b"late\n").unwrap_err();
    assert_eq!(f.version(), v, "the refused tool edit applied");
    assert_eq!(by_tool, ToolRunError::Session(past_cap()));
    assert_eq!(by_tool.exit(), 3);
    assert_eq!(by_tool.report().id(), "MPC-E305");
    // the person past the cap, through the same tool: the same refusal
    let by_person = f.tool(late_person, "append", b"late\n").unwrap_err();
    assert_eq!(by_tool, by_person, "tool and person refused alike");
    // its next verb shares its watching connection: still one watcher
    assert_eq!(f.join_cli("late-tool"), late_tool);
    assert_eq!(f.watching(first), 2);
    // the tool in its slot still edits
    let out = f.tool(t, "append", b"more\n").unwrap();
    assert!(out.changed.is_some());
}
