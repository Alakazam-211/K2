//! The node's job ledger (§11.5): SQLite, WAL, `synchronous=FULL`, a
//! random `ledger_id` so a replaced ledger can't inherit old jobs, and
//! `UNIQUE(job_id, generation)` so a stale or replayed attempt never runs.
//!
//! The node is the truth for execution; the controller is the truth for
//! intent. A non-terminal row whose `boot_id` isn't this boot's becomes
//! `interrupted/node_reboot` at start.
//!
//! On a shared node (prd-compute-shared-queue-v1 §7) an accepted job that
//! can't start yet is a row in state `queued` (reason `node_queued`). Those
//! rows survive a reboot and a `k2-node` restart (V47): only rows that had
//! started are interrupted. v3 adds `enqueued_at` (node clock), `class`,
//! `ws_id`, `session_id` and `est_secs`; an older binary ignores them.

use std::path::Path;

use k2_node_proto::frames::{Assign, ExitInfo, JobState, LedgerJob, Plan, SignedReceipt, SrcRan};
use k2_node_proto::results::Outputs;
use rusqlite::{params, Connection, OptionalExtension};

/// Time and boot identity, faked in tests.
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
    fn boot_id(&self) -> String;
}

pub struct SystemClock {
    boot: String,
}

impl SystemClock {
    pub fn new() -> Self {
        Self { boot: crate::sysinfo::boot_id() }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        crate::util::now()
    }
    fn boot_id(&self) -> String {
        self.boot.clone()
    }
}

/// One ledger row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub job_id: String,
    pub generation: u32,
    pub attempt: u32,
    pub plan_digest: String,
    pub plan: Plan,
    pub state: JobState,
    pub reason: Option<String>,
    pub detail: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub exit: Option<ExitInfo>,
    pub log_seq: u64,
    pub boot_id: String,
    pub src: Option<SrcRan>,
    pub receipt: Option<SignedReceipt>,
    pub cleaned: bool,
    /// Artifacts and test results (user_version 2).
    pub outputs: Option<Outputs>,
    /// When the node queued it (node clock; user_version 3). `None` for a
    /// job that started at once.
    pub enqueued_at: Option<i64>,
    /// Insert order in this ledger (SQLite rowid).
    pub seq: i64,
}

impl Row {
    pub fn to_ledger_job(&self) -> LedgerJob {
        LedgerJob {
            job_id: self.job_id.clone(),
            generation: self.generation,
            attempt: self.attempt,
            plan_digest: self.plan_digest.clone(),
            state: self.state,
            reason: self.reason.clone(),
            started_at: self.started_at,
            ended_at: self.ended_at,
            exit: self.exit.clone(),
            log_seq: self.log_seq,
            boot_id: self.boot_id.clone(),
        }
    }
}

pub struct Ledger {
    conn: Connection,
    pub ledger_id: String,
}

const COLS: &str = "job_id, generation, attempt, plan_digest, plan_json, state, reason, detail, created_at, \
    started_at, ended_at, exit_code, signal, log_seq, boot_id, src_json, receipt_json, cleaned, outputs_json, enqueued_at, rowid";

/// Terminal states, for SQL.
const TERMINAL: &str = "('done','failed','cancelled','timeout','interrupted','unknown')";

fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let plan_json: String = r.get(4)?;
    let state: String = r.get(5)?;
    let exit_code: Option<i32> = r.get(11)?;
    let signal: Option<i32> = r.get(12)?;
    let src_json: Option<String> = r.get(15)?;
    let receipt_json: Option<String> = r.get(16)?;
    let bad = |what: &str| rusqlite::Error::InvalidColumnType(0, what.to_string(), rusqlite::types::Type::Text);
    Ok(Row {
        job_id: r.get(0)?,
        generation: r.get(1)?,
        attempt: r.get(2)?,
        plan_digest: r.get(3)?,
        plan: serde_json::from_str(&plan_json).map_err(|_| bad("plan_json"))?,
        state: JobState::parse(&state).ok_or_else(|| bad("state"))?,
        reason: r.get(6)?,
        detail: r.get(7)?,
        created_at: r.get(8)?,
        started_at: r.get(9)?,
        ended_at: r.get(10)?,
        exit: if exit_code.is_some() || signal.is_some() { Some(ExitInfo { code: exit_code, signal }) } else { None },
        log_seq: r.get::<_, i64>(13)? as u64,
        boot_id: r.get(14)?,
        src: match src_json {
            Some(s) => Some(serde_json::from_str(&s).map_err(|_| bad("src_json"))?),
            None => None,
        },
        receipt: match receipt_json {
            Some(s) => Some(serde_json::from_str(&s).map_err(|_| bad("receipt_json"))?),
            None => None,
        },
        cleaned: r.get::<_, i64>(17)? != 0,
        // A row the node can't read back still reconciles; only the
        // outputs are lost.
        outputs: r.get::<_, Option<String>>(18)?.and_then(|s| serde_json::from_str(&s).ok()),
        enqueued_at: r.get(19)?,
        seq: r.get(20)?,
    })
}

fn sqlerr(e: rusqlite::Error) -> String {
    format!("ledger: {e}")
}

impl Ledger {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(sqlerr)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self, String> {
        Self::init(Connection::open_in_memory().map_err(sqlerr)?)
    }

    fn init(conn: Connection) -> Result<Self, String> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS jobs (
               job_id TEXT NOT NULL, generation INTEGER NOT NULL, attempt INTEGER NOT NULL,
               plan_digest TEXT NOT NULL, plan_json TEXT NOT NULL, state TEXT NOT NULL,
               reason TEXT, detail TEXT, created_at INTEGER NOT NULL, started_at INTEGER,
               ended_at INTEGER, exit_code INTEGER, signal INTEGER,
               log_seq INTEGER NOT NULL DEFAULT 0, pid INTEGER, boot_id TEXT NOT NULL,
               src_json TEXT, receipt_json TEXT, cleaned INTEGER NOT NULL DEFAULT 0,
               UNIQUE (job_id, generation)
             );
             PRAGMA user_version = 1;",
        )
        .map_err(sqlerr)?;
        // v2: artifacts + test results per attempt.
        let has_outputs = conn
            .prepare("SELECT 1 FROM pragma_table_info('jobs') WHERE name = 'outputs_json'")
            .and_then(|mut st| st.exists([]))
            .map_err(sqlerr)?;
        if !has_outputs {
            conn.execute_batch("ALTER TABLE jobs ADD COLUMN outputs_json TEXT;").map_err(sqlerr)?;
        }
        // v3: the node queue (shared nodes).
        let has_queue = conn
            .prepare("SELECT 1 FROM pragma_table_info('jobs') WHERE name = 'enqueued_at'")
            .and_then(|mut st| st.exists([]))
            .map_err(sqlerr)?;
        if !has_queue {
            conn.execute_batch(
                "ALTER TABLE jobs ADD COLUMN enqueued_at INTEGER;
                 ALTER TABLE jobs ADD COLUMN class TEXT;
                 ALTER TABLE jobs ADD COLUMN ws_id TEXT;
                 ALTER TABLE jobs ADD COLUMN session_id TEXT;
                 ALTER TABLE jobs ADD COLUMN est_secs INTEGER;",
            )
            .map_err(sqlerr)?;
        }
        conn.execute_batch("PRAGMA user_version = 3;").map_err(sqlerr)?;
        let id: Option<String> = conn
            .query_row("SELECT v FROM meta WHERE k = 'ledger_id'", [], |r| r.get(0))
            .optional()
            .map_err(sqlerr)?;
        let ledger_id = match id {
            Some(id) => id,
            None => {
                let id = k2_node_proto::crypto::random_id()?;
                conn.execute("INSERT INTO meta (k, v) VALUES ('ledger_id', ?1)", params![id]).map_err(sqlerr)?;
                id
            }
        };
        Ok(Self { conn, ledger_id })
    }

    /// Record a new attempt as `preparing`. Fails on a duplicate fence.
    pub fn insert(&self, a: &Assign, clock: &dyn Clock) -> Result<(), String> {
        let plan_json = serde_json::to_string(&a.plan).map_err(|e| format!("encode plan: {e}"))?;
        self.conn
            .execute(
                "INSERT INTO jobs (job_id, generation, attempt, plan_digest, plan_json, state, created_at, boot_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'preparing', ?6, ?7)",
                params![a.fence.job_id, a.fence.generation, a.fence.attempt, a.fence.plan_digest, plan_json, clock.now(), clock.boot_id()],
            )
            .map_err(sqlerr)?;
        Ok(())
    }

    /// Record an accepted job that waits in the node queue: state `queued`,
    /// reason `node_queued`. Fails on a duplicate fence.
    pub fn insert_queued(&self, a: &Assign, clock: &dyn Clock) -> Result<(), String> {
        let plan_json = serde_json::to_string(&a.plan).map_err(|e| format!("encode plan: {e}"))?;
        let class = a.plan.class.unwrap_or_default();
        let now = clock.now();
        self.conn
            .execute(
                "INSERT INTO jobs (job_id, generation, attempt, plan_digest, plan_json, state, reason, created_at, boot_id,
                   enqueued_at, class, ws_id, session_id, est_secs)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7, ?8, ?7, ?9, ?10, ?11, ?12)",
                params![
                    a.fence.job_id,
                    a.fence.generation,
                    a.fence.attempt,
                    a.fence.plan_digest,
                    plan_json,
                    k2_node_proto::share::codes::NODE_QUEUED,
                    now,
                    clock.boot_id(),
                    class.as_str(),
                    a.plan.workspace_id,
                    a.plan.session_id,
                    a.plan.est_secs.map(|e| e as i64),
                ],
            )
            .map_err(sqlerr)?;
        Ok(())
    }

    /// Every queued row, oldest first.
    pub fn queued(&self) -> Result<Vec<Row>, String> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {COLS} FROM jobs WHERE state = 'queued' ORDER BY enqueued_at, job_id, generation"))
            .map_err(sqlerr)?;
        let rows = st.query_map([], row_of).map_err(sqlerr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sqlerr)
    }

    /// Take a queued row out of the queue to start it now: `preparing`,
    /// this boot. False when it isn't queued any more (cancelled or
    /// withdrawn meanwhile): then it must not start.
    pub fn dequeue(&self, job_id: &str, generation: u32, clock: &dyn Clock) -> Result<bool, String> {
        let n = self
            .conn
            .execute(
                "UPDATE jobs SET state = 'preparing', reason = NULL, detail = NULL, boot_id = ?3
                 WHERE job_id = ?1 AND generation = ?2 AND state = 'queued'",
                params![job_id, generation, clock.boot_id()],
            )
            .map_err(sqlerr)?;
        Ok(n == 1)
    }

    /// End a row only if it's still queued (a withdraw, a cancel without
    /// `--force`, a superseded or revoked row). False when it had started:
    /// then nothing changed (V57).
    pub fn cancel_queued(&self, job_id: &str, generation: u32, reason: &str, detail: Option<&str>, at: i64) -> Result<bool, String> {
        let n = self
            .conn
            .execute(
                "UPDATE jobs SET state = 'cancelled', reason = ?3, detail = ?4, ended_at = ?5
                 WHERE job_id = ?1 AND generation = ?2 AND state = 'queued'",
                params![job_id, generation, reason, detail, at],
            )
            .map_err(sqlerr)?;
        Ok(n == 1)
    }

    pub fn get(&self, job_id: &str, generation: u32) -> Result<Option<Row>, String> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM jobs WHERE job_id = ?1 AND generation = ?2"),
                params![job_id, generation],
                row_of,
            )
            .optional()
            .map_err(sqlerr)
    }

    /// The highest generation the node has for `job_id`.
    pub fn latest(&self, job_id: &str) -> Result<Option<Row>, String> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM jobs WHERE job_id = ?1 ORDER BY generation DESC LIMIT 1"),
                params![job_id],
                row_of,
            )
            .optional()
            .map_err(sqlerr)
    }

    pub fn set_state(&self, job_id: &str, generation: u32, state: JobState, reason: Option<&str>, detail: Option<&str>) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE jobs SET state = ?3, reason = COALESCE(?4, reason), detail = COALESCE(?5, detail)
                 WHERE job_id = ?1 AND generation = ?2",
                params![job_id, generation, state.as_str(), reason, detail],
            )
            .map_err(sqlerr)?;
        Ok(())
    }

    pub fn set_running(&self, job_id: &str, generation: u32, pid: u32, src: Option<&SrcRan>, at: i64) -> Result<(), String> {
        let src_json = src.map(|s| serde_json::to_string(s).unwrap_or_default());
        self.conn
            .execute(
                "UPDATE jobs SET state = 'running', pid = ?3, src_json = ?4, started_at = ?5 WHERE job_id = ?1 AND generation = ?2",
                params![job_id, generation, pid, src_json, at],
            )
            .map_err(sqlerr)?;
        Ok(())
    }

    /// The process group leader's pid of an attempt (set when it ran).
    pub fn pid(&self, job_id: &str, generation: u32) -> Result<Option<i64>, String> {
        self.conn
            .query_row("SELECT pid FROM jobs WHERE job_id = ?1 AND generation = ?2", params![job_id, generation], |r| r.get::<_, Option<i64>>(0))
            .map_err(sqlerr)
    }

    pub fn set_src(&self, job_id: &str, generation: u32, src: &SrcRan) -> Result<(), String> {
        let src_json = serde_json::to_string(src).map_err(|e| format!("encode src: {e}"))?;
        self.conn
            .execute("UPDATE jobs SET src_json = ?3 WHERE job_id = ?1 AND generation = ?2", params![job_id, generation, src_json])
            .map_err(sqlerr)?;
        Ok(())
    }

    pub fn set_log_seq(&self, job_id: &str, generation: u32, seq: u64) -> Result<(), String> {
        self.conn
            .execute("UPDATE jobs SET log_seq = ?3 WHERE job_id = ?1 AND generation = ?2", params![job_id, generation, seq as i64])
            .map_err(sqlerr)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish(
        &self,
        job_id: &str,
        generation: u32,
        state: JobState,
        reason: Option<&str>,
        detail: Option<&str>,
        exit: Option<&ExitInfo>,
        log_seq: u64,
        at: i64,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE jobs SET state = ?3, reason = ?4, detail = ?5, exit_code = ?6, signal = ?7, log_seq = ?8,
                   ended_at = ?9, pid = NULL
                 WHERE job_id = ?1 AND generation = ?2",
                params![
                    job_id,
                    generation,
                    state.as_str(),
                    reason,
                    detail,
                    exit.and_then(|e| e.code),
                    exit.and_then(|e| e.signal),
                    log_seq as i64,
                    at
                ],
            )
            .map_err(sqlerr)?;
        Ok(())
    }

    pub fn set_receipt(&self, job_id: &str, generation: u32, r: &SignedReceipt) -> Result<(), String> {
        let j = serde_json::to_string(r).map_err(|e| format!("encode receipt: {e}"))?;
        self.conn
            .execute("UPDATE jobs SET receipt_json = ?3 WHERE job_id = ?1 AND generation = ?2", params![job_id, generation, j])
            .map_err(sqlerr)?;
        Ok(())
    }

    pub fn set_outputs(&self, job_id: &str, generation: u32, o: &Outputs) -> Result<(), String> {
        let j = serde_json::to_string(o).map_err(|e| format!("encode outputs: {e}"))?;
        self.conn
            .execute("UPDATE jobs SET outputs_json = ?3 WHERE job_id = ?1 AND generation = ?2", params![job_id, generation, j])
            .map_err(sqlerr)?;
        Ok(())
    }

    pub fn mark_cleaned(&self, job_id: &str, generation: u32) -> Result<(), String> {
        self.conn
            .execute("UPDATE jobs SET cleaned = 1 WHERE job_id = ?1 AND generation = ?2", params![job_id, generation])
            .map_err(sqlerr)?;
        Ok(())
    }

    /// Every row, newest first.
    pub fn all(&self) -> Result<Vec<Row>, String> {
        let mut st = self.conn.prepare(&format!("SELECT {COLS} FROM jobs ORDER BY created_at DESC, job_id")).map_err(sqlerr)?;
        let rows = st.query_map([], row_of).map_err(sqlerr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sqlerr)
    }

    pub fn non_terminal(&self) -> Result<Vec<Row>, String> {
        Ok(self.all()?.into_iter().filter(|r| !r.state.is_terminal()).collect())
    }

    /// Rows still open from another boot → `interrupted/node_reboot`.
    /// Queued rows never started, so they stay queued (V47).
    /// Returns the `(job_id, generation)` pairs changed.
    pub fn reconcile_boot(&self, clock: &dyn Clock) -> Result<Vec<(String, u32)>, String> {
        let boot = clock.boot_id();
        let mut out = Vec::new();
        for r in self.non_terminal()? {
            if r.boot_id != boot && r.state != JobState::Queued {
                self.finish(&r.job_id, r.generation, JobState::Interrupted, Some("node_reboot"), None, None, r.log_seq, clock.now())?;
                out.push((r.job_id, r.generation));
            }
        }
        Ok(out)
    }

    /// After a restart within the same boot: every attempt still open
    /// belonged to the previous `k2-node` process, which can't be
    /// re-adopted (its pipes are gone). Mark them `interrupted/<reason>`
    /// and return `(job_id, generation, pgid)` so the caller can kill
    /// what's left of each process group. Queued rows never started:
    /// they stay queued (V47).
    pub fn interrupt_open(&self, reason: &str, clock: &dyn Clock) -> Result<Vec<(String, u32, Option<i32>)>, String> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT job_id, generation, pid, log_seq FROM jobs WHERE state NOT IN {TERMINAL} AND state != 'queued'"))
            .map_err(sqlerr)?;
        let rows: Vec<(String, u32, Option<i64>, i64)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(sqlerr)?
            .collect::<Result<_, _>>()
            .map_err(sqlerr)?;
        drop(st);
        let mut out = Vec::new();
        for (job, gen, pid, seq) in rows {
            self.finish(&job, gen, JobState::Interrupted, Some(reason), None, None, seq as u64, clock.now())?;
            out.push((job, gen, pid.map(|p| p as i32)));
        }
        Ok(out)
    }

    /// Delete terminal rows that ended before `before`.
    pub fn prune(&self, before: i64) -> Result<usize, String> {
        self.conn
            .execute("DELETE FROM jobs WHERE ended_at IS NOT NULL AND ended_at < ?1", params![before])
            .map_err(sqlerr)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use k2_node_proto::frames::{Fence, JobLimits};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    pub struct FakeClock {
        pub t: Mutex<i64>,
        pub boot: Mutex<String>,
    }
    impl Clock for FakeClock {
        fn now(&self) -> i64 {
            *self.t.lock().unwrap()
        }
        fn boot_id(&self) -> String {
            self.boot.lock().unwrap().clone()
        }
    }

    pub fn assign(job: &str, generation: u32) -> Assign {
        let plan = Plan {
            class: None,
            session_id: None,
            est_secs: None,
            est_p90_secs: None,
            ws_max_running: None,
            job_id: job.into(),
            node_id: "n".into(),
            workspace_id: "w".into(),
            workspace_label: "k2".into(),
            requested_by: "agent:k2".into(),
            argv: vec!["true".into()],
            env: BTreeMap::new(),
            cwd: None,
            limits: JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 0, log_cap_bytes: 1 << 20 },
            src: None,
            exclusive: false,
            created_at: 1,
            needs: Vec::new(),
            artifacts: Vec::new(),
            stall_secs: None,
        };
        Assign { fence: Fence { job_id: job.into(), attempt: generation, generation, plan_digest: plan.digest().unwrap() }, plan }
    }

    #[test]
    fn unique_fence_and_latest_generation() {
        let clock = FakeClock { t: Mutex::new(10), boot: Mutex::new("b1".into()) };
        let l = Ledger::open_in_memory().unwrap();
        l.insert(&assign("j", 1), &clock).unwrap();
        assert!(l.insert(&assign("j", 1), &clock).is_err(), "a replayed fence is refused");
        l.insert(&assign("j", 2), &clock).unwrap();
        assert_eq!(l.latest("j").unwrap().unwrap().generation, 2);
        assert_eq!(l.get("j", 1).unwrap().unwrap().state, JobState::Preparing);
    }

    #[test]
    fn reboot_interrupts_open_rows_only() {
        let clock = FakeClock { t: Mutex::new(10), boot: Mutex::new("b1".into()) };
        let l = Ledger::open_in_memory().unwrap();
        l.insert(&assign("run", 1), &clock).unwrap();
        l.set_running("run", 1, 42, None, 11).unwrap();
        l.insert(&assign("done", 1), &clock).unwrap();
        l.finish("done", 1, JobState::Done, None, None, Some(&ExitInfo { code: Some(0), signal: None }), 3, 12).unwrap();
        assert!(l.reconcile_boot(&clock).unwrap().is_empty(), "same boot: nothing changes");
        *clock.boot.lock().unwrap() = "b2".into();
        assert_eq!(l.reconcile_boot(&clock).unwrap(), vec![("run".to_string(), 1)]);
        let r = l.get("run", 1).unwrap().unwrap();
        assert_eq!((r.state, r.reason.as_deref()), (JobState::Interrupted, Some("node_reboot")));
        assert_eq!(l.get("done", 1).unwrap().unwrap().exit.unwrap().code, Some(0));
    }

    #[test]
    fn restart_interrupts_what_the_old_process_left_open() {
        let clock = FakeClock { t: Mutex::new(10), boot: Mutex::new("b1".into()) };
        let l = Ledger::open_in_memory().unwrap();
        l.insert(&assign("a", 1), &clock).unwrap();
        l.set_running("a", 1, 4242, None, 11).unwrap();
        l.insert(&assign("b", 1), &clock).unwrap();
        l.finish("b", 1, JobState::Done, None, None, None, 0, 12).unwrap();
        let v = l.interrupt_open("node_restart", &clock).unwrap();
        assert_eq!(v, vec![("a".to_string(), 1, Some(4242))]);
        let r = l.get("a", 1).unwrap().unwrap();
        assert_eq!((r.state, r.reason.as_deref()), (JobState::Interrupted, Some("node_restart")));
        assert_eq!(l.get("b", 1).unwrap().unwrap().state, JobState::Done);
    }

    #[test]
    fn outputs_round_trip_and_an_old_ledger_gains_the_column() {
        let d = crate::util::temp_dir("ledger-v1");
        let path = d.join("l.sqlite");
        {
            // A version-1 ledger, as 0.45.x nodes wrote it.
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
                 CREATE TABLE jobs (job_id TEXT NOT NULL, generation INTEGER NOT NULL, attempt INTEGER NOT NULL,
                   plan_digest TEXT NOT NULL, plan_json TEXT NOT NULL, state TEXT NOT NULL, reason TEXT, detail TEXT,
                   created_at INTEGER NOT NULL, started_at INTEGER, ended_at INTEGER, exit_code INTEGER, signal INTEGER,
                   log_seq INTEGER NOT NULL DEFAULT 0, pid INTEGER, boot_id TEXT NOT NULL, src_json TEXT,
                   receipt_json TEXT, cleaned INTEGER NOT NULL DEFAULT 0, UNIQUE (job_id, generation));
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let clock = FakeClock { t: Mutex::new(10), boot: Mutex::new("b1".into()) };
        let l = Ledger::open(&path).unwrap();
        l.insert(&assign("j", 1), &clock).unwrap();
        assert_eq!(l.get("j", 1).unwrap().unwrap().outputs, None);
        let o = Outputs {
            artifacts: vec![k2_node_proto::results::ArtifactInfo { name: "a".into(), sha256: "s".into(), bytes: 1 }],
            tests: None,
            notes: vec!["n".into()],
        };
        l.set_outputs("j", 1, &o).unwrap();
        assert_eq!(l.get("j", 1).unwrap().unwrap().outputs, Some(o));
        drop(l);
        // Reopening a v2 ledger is a no-op.
        assert!(Ledger::open(&path).is_ok());
    }

    /// V47: queued rows survive a reboot and a restart; started ones don't.
    #[test]
    fn queued_rows_survive_reboot_and_restart() {
        let clock = FakeClock { t: Mutex::new(10), boot: Mutex::new("b1".into()) };
        let l = Ledger::open_in_memory().unwrap();
        let mut q = assign("q", 1);
        q.plan.class = Some(k2_node_proto::share::JobClass::Batch);
        q.plan.session_id = Some("s1".into());
        q.plan.est_secs = Some(90);
        q.fence.plan_digest = q.plan.digest().unwrap();
        l.insert_queued(&q, &clock).unwrap();
        assert!(l.insert_queued(&q, &clock).is_err(), "a replayed fence is refused");
        l.insert(&assign("r", 1), &clock).unwrap();
        l.set_running("r", 1, 7, None, 11).unwrap();
        let row = l.get("q", 1).unwrap().unwrap();
        assert_eq!((row.state, row.reason.as_deref(), row.enqueued_at), (JobState::Queued, Some("node_queued"), Some(10)));
        assert_eq!(row.plan.class, Some(k2_node_proto::share::JobClass::Batch));
        assert_eq!(l.interrupt_open("node_restart", &clock).unwrap().iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec!["r"]);
        *clock.boot.lock().unwrap() = "b2".into();
        assert!(l.reconcile_boot(&clock).unwrap().is_empty(), "r is already interrupted; q never started");
        assert_eq!(l.get("q", 1).unwrap().unwrap().state, JobState::Queued);
        assert_eq!(l.queued().unwrap().iter().map(|r| r.job_id.clone()).collect::<Vec<_>>(), vec!["q"]);
        // Starting it takes it out of the queue in this boot; a second
        // dequeue (a race) does nothing.
        assert!(l.dequeue("q", 1, &clock).unwrap());
        assert!(!l.dequeue("q", 1, &clock).unwrap());
        let row = l.get("q", 1).unwrap().unwrap();
        assert_eq!((row.state, row.reason, row.boot_id.as_str()), (JobState::Preparing, None, "b2"));
        // A started job can't be cancelled as queued (V57).
        assert!(!l.cancel_queued("q", 1, "withdrawn", None, 20).unwrap());
        assert_eq!(l.get("q", 1).unwrap().unwrap().state, JobState::Preparing);
        l.insert_queued(&assign("w", 1), &clock).unwrap();
        assert!(l.cancel_queued("w", 1, "withdrawn", None, 21).unwrap());
        let w = l.get("w", 1).unwrap().unwrap();
        assert_eq!((w.state, w.reason.as_deref(), w.ended_at), (JobState::Cancelled, Some("withdrawn"), Some(21)));
        assert!(l.queued().unwrap().is_empty());
    }

    #[test]
    fn a_v2_ledger_gains_the_queue_columns() {
        let d = crate::util::temp_dir("ledger-v2");
        let path = d.join("l.sqlite");
        let clock = FakeClock { t: Mutex::new(10), boot: Mutex::new("b1".into()) };
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
                 CREATE TABLE jobs (job_id TEXT NOT NULL, generation INTEGER NOT NULL, attempt INTEGER NOT NULL,
                   plan_digest TEXT NOT NULL, plan_json TEXT NOT NULL, state TEXT NOT NULL, reason TEXT, detail TEXT,
                   created_at INTEGER NOT NULL, started_at INTEGER, ended_at INTEGER, exit_code INTEGER, signal INTEGER,
                   log_seq INTEGER NOT NULL DEFAULT 0, pid INTEGER, boot_id TEXT NOT NULL, src_json TEXT,
                   receipt_json TEXT, cleaned INTEGER NOT NULL DEFAULT 0, outputs_json TEXT, UNIQUE (job_id, generation));
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        }
        let l = Ledger::open(&path).unwrap();
        l.insert(&assign("old", 1), &clock).unwrap();
        assert_eq!(l.get("old", 1).unwrap().unwrap().enqueued_at, None);
        l.insert_queued(&assign("new", 1), &clock).unwrap();
        assert_eq!(l.get("new", 1).unwrap().unwrap().enqueued_at, Some(10));
        drop(l);
        assert!(Ledger::open(&path).is_ok(), "reopening a v3 ledger is a no-op");
    }

    #[test]
    fn ledger_id_survives_reopen() {
        let d = crate::util::temp_dir("ledger");
        let a = Ledger::open(&d.join("l.sqlite")).unwrap().ledger_id;
        let b = Ledger::open(&d.join("l.sqlite")).unwrap().ledger_id;
        assert_eq!(a, b);
        let c = Ledger::open(&d.join("other.sqlite")).unwrap().ledger_id;
        assert_ne!(a, c, "a replaced ledger gets a new id");
    }
}
