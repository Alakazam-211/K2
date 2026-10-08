//! The node's job ledger (§11.5): SQLite, WAL, `synchronous=FULL`, a
//! random `ledger_id` so a replaced ledger can't inherit old jobs, and
//! `UNIQUE(job_id, generation)` so a stale or replayed attempt never runs.
//!
//! The node is the truth for execution; the controller is the truth for
//! intent. A non-terminal row whose `boot_id` isn't this boot's becomes
//! `interrupted/node_reboot` at start.

use std::path::Path;

use k2_node_proto::frames::{Assign, ExitInfo, JobState, LedgerJob, Plan, SignedReceipt, SrcRan};
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
    started_at, ended_at, exit_code, signal, log_seq, boot_id, src_json, receipt_json, cleaned";

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
    /// Returns the `(job_id, generation)` pairs changed.
    pub fn reconcile_boot(&self, clock: &dyn Clock) -> Result<Vec<(String, u32)>, String> {
        let boot = clock.boot_id();
        let mut out = Vec::new();
        for r in self.non_terminal()? {
            if r.boot_id != boot {
                self.finish(&r.job_id, r.generation, JobState::Interrupted, Some("node_reboot"), None, None, r.log_seq, clock.now())?;
                out.push((r.job_id, r.generation));
            }
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
    fn ledger_id_survives_reopen() {
        let d = crate::util::temp_dir("ledger");
        let a = Ledger::open(&d.join("l.sqlite")).unwrap().ledger_id;
        let b = Ledger::open(&d.join("l.sqlite")).unwrap().ledger_id;
        assert_eq!(a, b);
        let c = Ledger::open(&d.join("other.sqlite")).unwrap().ledger_id;
        assert_ne!(a, c, "a replaced ledger gets a new id");
    }
}
