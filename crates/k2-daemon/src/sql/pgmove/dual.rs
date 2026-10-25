//! Live dual-run: `k2 db upgrade <db> --live` (prd-app-pg-choice-dualrun-v1
//! Part B, D2; 0.46.4).
//!
//! The target database on the new major gets the schema only; logical
//! replication then copies every row from the live source while the Apps
//! keep serving, DDL frozen. The owner (or, later, the agent) runs the
//! cut-over: writes blocked a few seconds, the stream drained, sequences
//! copied, verified, the record moved, the Apps re-hosted on the new DSN.
//! Before the switch an abort is free (the source never stopped); after
//! it, the 0.46.3 rollback applies (the source is kept closed 14 days).
//!
//! - The run lives in `sql_dual_runs` (migration 0222), never in
//!   `sql_moves`: a run lasts days, and boot recovery finishes or undoes
//!   every open move within 30 minutes (L4). The cut-over is an ordinary
//!   `sql_moves` row with `method = 'dual'` that the 0.46.3 recovery reads.
//! - A streaming run doesn't hold the move worker (L5): only setup, the
//!   cut-over and an abort take it, so the re-host wave and other moves go on.
//! - The machine runs over [`DualOps`] (the root helper's protocol-10
//!   `pgdual` verbs in production), the move's [`ClusterOps`] and
//!   [`MoveEnv`], and [`RunStore`] (the run rows): every step is testable
//!   without Postgres.
//! - Owner-only for the first release ([`DUAL_LIVE_AGENTS`], §13): agents
//!   get it, with no migration, once the first owner runs are clean.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use k2_root_proto::pg_dual::DualOp;

use super::check::{self, CheckLine, Level, Report};
use super::{
    catalog, fingerprint, free_band_port, new_progress, ok_name, row_counts, ClusterOps, ConnMode, DbRecord, Major, MoveEnv,
    MoveOpts, MoveProgress, MoveState, MovedInfo, Run, SourceState, Step, StepFail, NOTICE_SECS, ROLLBACK_WINDOW_SECS,
};

/// §13: `--live`, `--switch` and `--abort` are the owner's until the first
/// owner-run moves are clean; then this flips (no migration).
pub const DUAL_LIVE_AGENTS: bool = false;
/// PA30: a run older than this is aborted (the freeze and WAL can't hold
/// forever); the inbox warns at day 3 and day 6.
pub const RUN_MAX_SECS: i64 = 7 * 86_400;
pub const WARN_AT_DAYS: [i64; 2] = [3, 6];
/// PA31 preconditions: at most this much lag to start a switch.
pub const SWITCH_MAX_LAG: u64 = 16 << 20;
/// PA29: inbox once per episode past this lag, or no progress this long.
pub const LAG_WARN: u64 = 256 << 20;
pub const NO_PROGRESS_SECS: i64 = 600;
/// PA31 step 3: the drain's wait.
pub const DRAIN_WAIT_SECS: u64 = 60;
/// PA29: the watch loop's period.
pub const WATCH_EVERY_SECS: u64 = 30;
/// PA22: the slot's WAL cap, at most 8 GB.
pub const WAL_KEEP_MAX_MB: u32 = 8192;
/// Security review B4: the cut-over's materialized-view refresh runs
/// under the write block, so it is bounded (default 10 minutes; the
/// daemon's `K2_DB_LIVE_REFRESH_TIMEOUT_SECS` overrides it, 30 s–24 h).
/// Past it the cut-over is undone.
pub const REFRESH_TIMEOUT_SECS: u64 = 600;

pub fn refresh_timeout_secs(o: Option<u64>) -> u64 {
    o.or_else(|| std::env::var("K2_DB_LIVE_REFRESH_TIMEOUT_SECS").ok().and_then(|v| v.trim().parse().ok()))
        .unwrap_or(REFRESH_TIMEOUT_SECS)
        .clamp(30, 86_400)
}
/// The refusal line ids of `--check --live`.
pub const SOURCE_NOT_LOGICAL: &str = "source_not_logical";
pub const DUAL_RUNNING: &str = "dual_running";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    SettingUp,
    Syncing,
    Streaming,
    Stalled,
    Switching,
    Switched,
    Aborted,
    Failed,
    /// Security review B7: the run ended (aborted, failed) but the root
    /// helper couldn't tear down the source's slot and DDL freeze (an old
    /// or unreachable helper). It stays open, so K2 keeps refusing
    /// `db_dual_running`, until the watch loop (or another `--abort`)
    /// finishes the teardown.
    NeedsHelper,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SettingUp => "setting_up",
            Self::Syncing => "syncing",
            Self::Streaming => "streaming",
            Self::Stalled => "stalled",
            Self::Switching => "switching",
            Self::Switched => "switched",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
            Self::NeedsHelper => "needs_helper",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(Value::String(s.into())).ok()
    }
    /// Not finished: the one-open-run index counts it.
    pub fn open(self) -> bool {
        !matches!(self, Self::Switched | Self::Aborted | Self::Failed)
    }
    /// The watch loop looks at it.
    fn watched(self) -> bool {
        matches!(self, Self::Syncing | Self::Streaming | Self::Stalled)
    }
}

/// What the run made (and so what an abort drops).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Objects {
    /// The 12-hex suffix of every object (`k2_dual_<id>`, `k2_repl_<id>`).
    pub object_id: String,
    /// The schema dump's tag.
    pub tag: String,
    pub wal_keep_mb: u32,
    pub target_port: Option<u16>,
    pub target_created: bool,
    pub source_setup: bool,
    pub subscribed: bool,
    /// The cut-over detached the stream (from here an undo drops the target).
    pub detached: bool,
    /// Tables whose FORCE RLS stays off on the target until the switch.
    pub forced_oids: Vec<u32>,
    /// A run waiting for the helper ([`RunState::NeedsHelper`]): how it
    /// ends once the teardown runs, and why it ended.
    #[serde(default)]
    pub end_as: Option<RunState>,
    #[serde(default)]
    pub end_why: Option<String>,
    /// Roles the setup created on the target (dropped by an undo).
    #[serde(default)]
    pub roles_created: Vec<String>,
}

/// The watch loop's last sample (PA29).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lag {
    pub at: i64,
    pub tables_ready: u64,
    pub tables_total: u64,
    pub lag_bytes: Option<u64>,
    pub wal_status: Option<String>,
    pub enabled: Option<bool>,
    pub apply_errors: u64,
    pub sync_errors: u64,
    pub confirmed_lsn: Option<String>,
    pub last_progress_at: i64,
    pub errors: Vec<String>,
    /// Inbox episodes already told (`lag`, `unreserved`, `stalled`,
    /// `day3`, `day6`, `streaming`).
    pub told: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DualRun {
    pub id: String,
    pub database_id: String,
    pub db: String,
    pub from: Major,
    pub to: Major,
    pub state: RunState,
    pub objects: Objects,
    /// Tables set `REPLICA IDENTITY FULL` by the owner's opt-in (Q10).
    pub replica_identity_full: Vec<String>,
    pub lag: Lag,
    pub move_id: Option<String>,
    pub actor: String,
    pub started_at: i64,
    pub streaming_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub last_error: Option<String>,
}

impl DualRun {
    /// `GET /cli/db/upgrade` `live` (PA29).
    pub fn json(&self) -> Value {
        json!({
            "id": self.id,
            "db": self.db,
            "from": self.from,
            "to": self.to,
            "state": self.state,
            "tables": {"ready": self.lag.tables_ready, "total": self.lag.tables_total},
            "lagBytes": self.lag.lag_bytes,
            "lagSecs": if self.lag.last_progress_at > 0 { Some((self.lag.at - self.lag.last_progress_at).max(0)) } else { None },
            "walStatus": self.lag.wal_status,
            "errors": {"apply": self.lag.apply_errors, "sync": self.lag.sync_errors, "read": self.lag.errors},
            "startedAt": self.started_at,
            "streamingAt": self.streaming_at,
            "finishedAt": self.finished_at,
            "lastError": self.last_error,
            "move": self.move_id,
            "actor": self.actor,
            "replicaIdentityFull": self.replica_identity_full,
            "expiresAt": self.started_at + RUN_MAX_SECS,
        })
    }
}

/// The objects' 12-hex id: the first 12 hex digits of the database's K2
/// id (PA23), so names stay ≤ 63 bytes whatever the database is called.
pub fn object_id(database_id: &str) -> String {
    let hex: String = database_id.chars().filter(|c| c.is_ascii_hexdigit()).map(|c| c.to_ascii_lowercase()).collect();
    if hex.len() >= 12 {
        return hex[..12].to_string();
    }
    // Not a UUID (tests, old rows): a stable hash of it.
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(database_id.as_bytes());
    d.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// PA22: `min(8 GB, 25% of free space)` in MB, at least the helper's floor.
pub fn wal_keep_mb(free_bytes: u64) -> u32 {
    let quarter = (free_bytes / 4) >> 20;
    (quarter.min(u64::from(WAL_KEEP_MAX_MB)) as u32).max(k2_root_proto::pg_dual::WAL_KEEP_MB.0)
}

// ── seams ───────────────────────────────────────────────────────────────

/// The root helper's protocol-10 `pgdual` verbs (and the owner's own
/// login for the materialized views).
pub trait DualOps {
    /// The helper speaks protocol 10 with `namespaces.pgdual` now
    /// (rechecked: a helper can be updated, or rolled back, under a run).
    fn ready(&self) -> Result<(), String>;
    fn logical_conf(&self, m: Major, wal_keep_mb: Option<u32>, if_logical: bool) -> Result<Value, String>;
    fn dump_schema(&self, from: Major, to: Major, db: &str, tag: &str) -> Result<u64, String>;
    /// `move-restore --keep-force-off` as the owner: the tables that had FORCE.
    fn restore_keep_force_off(&self, to: Major, db: &str, tag: &str, role: &str, password: &str) -> Result<Vec<u32>, String>;
    fn force_rls(&self, m: Major, db: &str, oids: &[u32]) -> Result<(), String>;
    fn dual(&self, op: DualOp, from: Major, to: Major, db: &str, id: &str, password: Option<&str>) -> Result<Value, String>;
    /// `pg systemctl restart postgresql@<M>-main` (the owner's `--prepare-live`).
    fn restart_cluster(&self, m: Major) -> Result<(), String>;
    /// `REFRESH MATERIALIZED VIEW` of every matview as the owner, over
    /// loopback with its own password (never the superuser), stopped
    /// after `timeout_secs` (B4).
    fn refresh_matviews(&self, m: Major, port: u16, db: &str, role: &str, password: &str, timeout_secs: u64) -> Result<(), String>;
}

/// The run rows (`sql_dual_runs`) and the bits of K2 only a run needs.
pub trait RunStore {
    fn open_run(&self, db: &str) -> Option<DualRun>;
    fn open_runs(&self) -> Vec<DualRun>;
    fn last_run(&self, db: &str) -> Option<DualRun>;
    /// The newest run whose objects carry this id (the orphan sweep).
    fn run_by_object(&self, object_id: &str) -> Option<DualRun>;
    /// A new open run: refused while the database has one (the index).
    fn insert_run(&self, r: &DualRun) -> Result<(), String>;
    fn save_run(&self, r: &DualRun) -> Result<(), String>;
    /// Save only while the stored row is still in `expect` (the watch loop
    /// never overwrites an abort or a switch that started meanwhile).
    /// `Ok(false)` = someone else moved it; nothing written.
    fn save_run_if(&self, r: &DualRun, expect: RunState) -> Result<bool, String> {
        if self.last_run(&r.db).filter(|x| x.id == r.id).map(|x| x.state) != Some(expect) {
            return Ok(false);
        }
        self.save_run(r).map(|_| true)
    }
    /// 32 random bytes, base64url: the replication role's password (never
    /// stored, never logged).
    fn new_password(&self) -> Result<String, String>;
    fn database_id(&self, db: &str) -> Result<String, String>;
    /// The state of a cut-over's `sql_moves` row.
    fn move_state(&self, move_id: &str) -> Option<MoveState>;
    /// The move worker, held while the watch loop ends a run (security
    /// review B6): a `--switch` or `--abort` holds it too, so the two never
    /// undo one run at once. `Err` = busy; the loop tries again next tick.
    fn hold_worker(&self, db: &str) -> Result<Box<dyn std::any::Any>, String>;
}

/// What the caller asked for.
#[derive(Debug, Clone, Default)]
pub struct LiveOpts {
    pub base: MoveOpts,
    /// Owner only: `REPLICA IDENTITY FULL` on the tables without a key.
    pub replica_identity_full: bool,
    /// Helper protocol ≥ 10 with `namespaces.pgdual`.
    pub dual_ok: bool,
}

/// The workspace's agent and the server's owner both hear a live run (PA29).
fn note(env: &dyn MoveEnv, title: &str, body: &str) {
    env.inbox(title, body);
    env.owner_note(title, body);
}

fn line(id: &str, level: Level, text: impl Into<String>, hint: Option<&str>) -> CheckLine {
    CheckLine { id: id.into(), level, text: text.into(), hint: hint.map(str::to_string) }
}

fn list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn head(v: &[String]) -> String {
    let mut s = v.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
    if v.len() > 10 {
        s.push_str(&format!(" and {} more", v.len() - 10));
    }
    s
}

// ── --check --live (PA19, PA20) ─────────────────────────────────────────

/// The copy path's report plus the live lines. `Err` only when a read
/// failed. The facts' `noPk` list comes back in `.1` (the setup needs it).
pub fn check_live(ops: &dyn ClusterOps, env: &dyn MoveEnv, store: &dyn RunStore, rec: &DbRecord, o: &LiveOpts) -> Result<(Report, Vec<String>), String> {
    let mut r = check::run(ops, env, rec, &o.base)?;
    let (db, from, to) = (rec.name.as_str(), rec.major, r.to);
    let mut l: Vec<CheckLine> = Vec::new();
    if o.dual_ok {
        l.push(line("helper", Level::Ok, "root helper speaks live moves (protocol 10, pgdual)", None));
    } else {
        l.push(line(
            "helper",
            Level::Refuse,
            "helper_old: the root helper doesn't speak live moves (protocol 10, pgdual)",
            Some("update the root helper (k2 root-helper update); a move by copy still works"),
        ));
    }
    if store.open_run(db).is_some() {
        l.push(line(DUAL_RUNNING, Level::Refuse, format!("a live move of {db} is already running"), Some("k2 db upgrade <db> --switch or --abort it")));
    }
    let online = ops.clusters()?.iter().any(|c| c.major == from && c.name == "main" && c.online());
    let mut no_pk = Vec::new();
    if online {
        let wal = ops.sql(from, "postgres", "SHOW wal_level;\n")?;
        if wal.trim() == "logical" {
            l.push(line("logical", Level::Ok, format!("Postgres {from} runs with wal_level = logical"), None));
        } else {
            l.push(line(
                SOURCE_NOT_LOGICAL,
                Level::Refuse,
                format!("Postgres {from} runs with wal_level = {}; live moves need logical", wal.trim()),
                Some("the owner prepares it once: k2 db upgrade --prepare-live <major> (every database on it pauses about 5-15 s)"),
            ));
        }
        let facts: Value = serde_json::from_str(&ops.sql(from, db, &catalog::live_facts_sql())?).map_err(|_| "live facts are not JSON".to_string())?;
        no_pk = list(&facts["noPk"]);
        if !no_pk.is_empty() {
            if o.replica_identity_full {
                l.push(line(
                    "replica_identity",
                    Level::Warn,
                    format!("REPLICA IDENTITY FULL is set on {} table(s) without a primary key: {}", no_pk.len(), head(&no_pk)),
                    Some("updates and deletes on those tables get slower (the whole row identifies it)"),
                ));
            } else {
                l.push(line(
                    "no_primary_key",
                    Level::Refuse,
                    format!("{} table(s) have no primary key: {}", no_pk.len(), head(&no_pk)),
                    Some("add a primary key in a migration, or the owner opts in: --replica-identity-full (slower updates on them)"),
                ));
            }
        }
        let n = |k: &str| facts[k].as_u64().unwrap_or(0);
        if n("largeObjects") > 0 {
            l.push(line("large_objects", Level::Refuse, format!("{} large objects (logical replication can't carry them)", n("largeObjects")), Some("use the move by copy")));
        }
        let unlogged = list(&facts["unlogged"]);
        if !unlogged.is_empty() {
            l.push(line(
                "unlogged",
                Level::Refuse,
                format!("unlogged tables: {}", head(&unlogged)),
                Some("ALTER TABLE … SET LOGGED in a migration, or use the move by copy"),
            ));
        }
        if n("foreign") > 0 {
            l.push(line("foreign_tables", Level::Refuse, format!("{} foreign tables", n("foreign")), Some("use the move by copy")));
        }
        if n("matviews") > 0 {
            l.push(line("matviews", Level::Warn, format!("{} materialized view(s): refreshed as the owner at the switch", n("matviews")), Some("they are empty on the new copy until then")));
        }
        if n("partitioned") > 0 {
            l.push(line("partitions", Level::Ok, format!("{} partitioned table(s): published leaf to leaf", n("partitioned")), None));
        }
        if n("generated") > 0 {
            l.push(line("generated", Level::Warn, format!("{} generated column(s): recomputed on the new copy", n("generated")), Some("fine unless a generated expression differs between majors")));
        }
    }
    // Disk: the copy (2 × size + 1 GiB, already a line) plus the WAL cap.
    let free = ops.free_bytes()?;
    let cap = u64::from(wal_keep_mb(free)) << 20;
    let need = catalog::disk_needed(r.size_bytes).saturating_add(cap);
    if free >= need {
        l.push(line("disk", Level::Ok, format!("{} MiB free; the copy and a {} MiB WAL cap need {} MiB", free >> 20, cap >> 20, need >> 20), None));
    } else {
        l.push(line("disk", Level::Refuse, format!("{} MiB free; the copy and the WAL cap need {} MiB", free >> 20, need >> 20), Some("free disk space first")));
    }
    let first_sync = r.size_bytes / (40 << 20);
    l.push(line(
        "copy",
        Level::Warn,
        format!("the first copy takes about {first_sync} s ({} MiB at ~40 MB/s) while the Apps keep serving", r.size_bytes >> 20),
        Some("schema changes (DDL, temp tables included) are refused on this database for the whole run (at most 7 days)"),
    ));
    l.push(line(
        "pause",
        Level::Ok,
        "writes pause for a few seconds at the switch (drain, sequences, a full row count), plus each App's own restart",
        None,
    ));
    if !r.users.apps.is_empty() {
        l.push(line("users", Level::Ok, format!("Apps keep serving during the copy and are re-hosted on Postgres {to} at the switch: {}", r.users.apps.join(", ")), None));
    }
    // The copy path's lines that don't hold for a live run.
    r.lines.retain(|x| !(x.id == "pause" || (x.id == "users" && x.text.starts_with("Apps stopped and restarted")) || x.id == "disk" && x.level != Level::Refuse));
    r.lines.extend(l);
    Ok((r, no_pk))
}

// ── start (PA21–PA28) ───────────────────────────────────────────────────

fn save(store: &dyn RunStore, r: &DualRun) -> Result<(), String> {
    store.save_run(r).map_err(|e| format!("run row: {e}"))
}

/// `k2 db upgrade <db> --live`: check, then set the run up; returns once
/// it streams its first copy (`syncing`). Any failure undoes what was made
/// (PA34) and records the run `aborted`.
pub fn start(ops: &dyn ClusterOps, dops: &dyn DualOps, env: &dyn MoveEnv, store: &dyn RunStore, db: &str, o: &LiveOpts) -> Result<DualRun, String> {
    let rec = env.record(db)?;
    let (report, no_pk) = check_live(ops, env, store, &rec, o)?;
    if report.refused() {
        let why: Vec<String> = report.lines.iter().filter(|l| l.level == Level::Refuse).map(|l| format!("{}: {}", l.id, l.text)).collect();
        return Err(format!("refused: {}", why.join("; ")));
    }
    let database_id = store.database_id(db)?;
    let now = env.now();
    let tag = super::tag_for(now);
    let free = ops.free_bytes()?;
    let mut run = DualRun {
        id: format!("dr_{db}_{}", tag.to_ascii_lowercase()),
        database_id: database_id.clone(),
        db: db.to_string(),
        from: rec.major,
        to: report.to,
        state: RunState::SettingUp,
        objects: Objects { object_id: object_id(&database_id), tag, wal_keep_mb: wal_keep_mb(free), ..Default::default() },
        replica_identity_full: if o.replica_identity_full { no_pk } else { Vec::new() },
        lag: Lag::default(),
        move_id: None,
        actor: o.base.actor.clone(),
        started_at: now,
        streaming_at: None,
        finished_at: None,
        last_error: None,
    };
    store.insert_run(&run).map_err(|e| format!("{DUAL_RUNNING}: {e}"))?;
    match setup(ops, dops, env, store, &mut run, &report) {
        Ok(()) => {
            run.state = RunState::Syncing;
            save(store, &run)?;
            env.audit(
                "db-upgrade-live-start",
                json!({"db": db, "from": run.from, "to": run.to, "actor": run.actor, "run": run.id, "replicaIdentityFull": run.replica_identity_full}),
            );
            note(env, 
                &format!("Database {db} live move started"),
                &format!(
                    "{} started copying {db} to Postgres {} while it keeps serving. Schema changes are refused on {db} until the switch \
                     or an abort (at most 7 days). Watch: k2 db upgrade {db}; then k2 db upgrade {db} --switch (or --abort).",
                    run.actor, run.to
                ),
            );
            Ok(run)
        }
        Err(why) => {
            let done = abort_run(ops, dops, env, store, run, &format!("setup: {why}"), false);
            Err(format!("setup_failed: {why}{}", done.last_error.as_deref().filter(|e| e.contains("undo incomplete")).map(|e| format!(" ({e})")).unwrap_or_default()))
        }
    }
}

fn setup(ops: &dyn ClusterOps, dops: &dyn DualOps, env: &dyn MoveEnv, store: &dyn RunStore, run: &mut DualRun, report: &Report) -> Result<(), String> {
    let (db, from, to) = (run.db.clone(), run.from, run.to);
    // PA22/PA24: the slot's WAL cap and the hba guard, never a wal_level
    // change (the check proved the cluster logical).
    dops.logical_conf(from, Some(run.objects.wal_keep_mb), true).map_err(|e| format!("logical-conf: {e}"))?;
    // The target cluster.
    let to_port = match report.to_port {
        Some(p) => p,
        None => free_band_port(&ops.clusters()?, &ops.band_listening()?).ok_or("band_full: no free port in 5432–5449")?,
    };
    if !ops.installed()?.contains(&to) {
        ops.add_major(to).map_err(|e| format!("add-major {to}: {e}"))?;
    }
    ops.ensure_cluster(to, to_port).map_err(|e| format!("ensure-cluster {to}: {e}"))?;
    env.record_cluster(to, to_port);
    run.objects.target_port = Some(to_port);
    let missing: Vec<String> = report.extensions.iter().filter(|x| ops.extension_available(to, x) != Some(true)).cloned().collect();
    if !missing.is_empty() {
        return Err(format!("extension_missing: no {to} build of {}", missing.join(", ")));
    }
    // Roles with their verifiers, then memberships (as the copy path).
    let out = ops.copy_roles(from, to, &report.roles, false).map_err(|e| format!("copy roles: {e}"))?;
    run.objects.roles_created = out["created"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    save(store, run).map_err(|e| format!("run row: {e}"))?;
    let m = ops.sql(from, &db, &catalog::memberships_sql(&report.roles))?;
    let grants = catalog::membership_grants(&m, ok_name).ok_or("memberships: unexpected answer")?;
    if !grants.is_empty() {
        ops.sql(to, "postgres", &grants).map_err(|e| format!("memberships: {e}"))?;
    }
    // Q10: the owner's opt-in, before the publication exists.
    if !run.replica_identity_full.is_empty() {
        ops.sql(from, &db, &catalog::replica_identity_sql(&run.replica_identity_full, true)).map_err(|e| format!("replica identity: {e}"))?;
        save(store, run)?;
    }
    // PA25: the schema only, FORCE kept off for the apply worker.
    let props: Value = serde_json::from_str(&ops.sql(from, &db, catalog::db_props_sql())?).map_err(|_| "database properties are not JSON")?;
    let acl = ops.sql(from, &db, catalog::datacl_sql())?;
    let settings = ops.sql(from, &db, catalog::db_settings_sql())?;
    let owner = props["owner"].as_str().ok_or("the database has no owner")?.to_string();
    let logins = env.logins(&db)?;
    let owner_pw = logins.iter().find(|(r, _)| *r == owner).map(|(_, p)| p.clone()).ok_or("the database's owner is not K2's migrator")?;
    let tag = run.objects.tag.clone();
    dops.dump_schema(from, to, &db, &tag).map_err(|e| format!("schema dump: {e}"))?;
    let create = catalog::create_db_sql(&db, &props, ok_name).ok_or("the database's owner or locale can't be carried")?;
    run.objects.target_created = true;
    save(store, run)?;
    ops.sql(to, "postgres", &create).map_err(|e| format!("create the target database: {e}"))?;
    run.objects.forced_oids = dops.restore_keep_force_off(to, &db, &tag, &owner, &owner_pw).map_err(|e| format!("schema restore: {e}"))?;
    save(store, run)?;
    let replay = catalog::datacl_replay(&db, &acl, ok_name).ok_or("database ACL: unexpected answer")?
        + &catalog::db_settings_replay(&db, &settings, ok_name).ok_or("database settings: unexpected answer")?;
    if !replay.is_empty() {
        ops.sql(to, "postgres", &replay).map_err(|e| format!("database grants: {e}"))?;
    }
    // PA25 step 4: nothing but K2 and the apply worker writes there.
    ops.allow_conn(to, &db, ConnMode::Quiesce).map_err(|e| format!("quiesce the target: {e}"))?;
    // PA23, PA26, PA27: one password, made here, on stdin twice, then gone.
    let pw = store.new_password()?;
    let id = run.objects.object_id.clone();
    run.objects.source_setup = true;
    save(store, run)?;
    dops.dual(DualOp::SetupSource, from, to, &db, &id, Some(&pw)).map_err(|e| format!("setup-source: {e}"))?;
    run.objects.subscribed = true;
    save(store, run)?;
    dops.dual(DualOp::Subscribe, from, to, &db, &id, Some(&pw)).map_err(|e| format!("subscribe: {e}"))?;
    drop(pw);
    if let Err(e) = ops.remove_dump(&db, &tag) {
        run.lag.errors.push(format!("remove the schema dump: {e}"));
    }
    Ok(())
}

// ── abort and cleanup (PA34) ────────────────────────────────────────────

/// Drop whatever the run made, in an order that works whatever exists.
/// Answers what couldn't be undone.
fn undo_objects(ops: &dyn ClusterOps, dops: &dyn DualOps, run: &mut DualRun) -> Vec<String> {
    let mut errs = Vec::new();
    let (db, from, to, id) = (run.db.clone(), run.from, run.to, run.objects.object_id.clone());
    if run.objects.source_setup || run.objects.subscribed {
        match dops.ready().and_then(|_| dops.dual(DualOp::Teardown, from, to, &db, &id, None)) {
            Ok(_) => {
                run.objects.source_setup = false;
                run.objects.subscribed = false;
            }
            Err(e) => {
                // B7: the slot and freeze stay; so does the target (its
                // subscription keeps DROP DATABASE out). Nothing else yet.
                errs.push(format!("teardown: {e}"));
                return errs;
            }
        }
    }
    if !run.replica_identity_full.is_empty() && !run.objects.detached {
        if let Err(e) = ops.sql(from, &db, &catalog::replica_identity_sql(&run.replica_identity_full, false)) {
            run.lag.errors.push(format!("replica identity back to default: {e}"));
        }
    }
    if run.objects.target_created {
        // The helper drops only a quiesced database.
        match ops.allow_conn(to, &db, ConnMode::Quiesce).and_then(|_| ops.drop_db(to, &db)) {
            Ok(_) => run.objects.target_created = false,
            Err(e) => errs.push(format!("drop the target copy: {e}")),
        }
    }
    // The roles the setup created on the target (0.46.4 drill: left
    // behind, they blocked the next move of a database with this name).
    if !run.objects.target_created && !run.objects.roles_created.is_empty() {
        match catalog::drop_roles_sql(&run.objects.roles_created, super::ok_name) {
            Some(sql) => match ops.sql(to, "postgres", &sql) {
                Ok(_) => run.objects.roles_created.clear(),
                Err(e) => errs.push(format!("drop the roles made on {to}: {e}")),
            },
            None => errs.push("roles made on the target: not role names".into()),
        }
    }
    if !run.objects.tag.is_empty() {
        if let Err(e) = ops.remove_dump(&db, &run.objects.tag) {
            run.lag.errors.push(format!("remove the schema dump: {e}"));
        }
    }
    errs
}

/// After a cut-over undone before the detach (0.46.4 drill): a walsender
/// ended in the write block makes Postgres disable the subscription
/// (`disable_on_error`), and nothing would enable it again. Enabled here,
/// then read back. `Err` = it can't stream on. An unreadable status is
/// left to the watch loop (never a guess).
fn resume_stream(ops: &dyn ClusterOps, dops: &dyn DualOps, run: &DualRun) -> Result<(), String> {
    let Ok(st) = status_now(dops, run) else { return Ok(()) };
    let tgt = &st["target"];
    if tgt["subscription"].as_bool() == Some(false) {
        return Err("the subscription is gone".into());
    }
    if tgt["enabled"].as_bool() != Some(false) {
        return Ok(());
    }
    let n = k2_root_proto::pg_dual::DualNames::of(&run.objects.object_id).ok_or("the run's id is not 12 hex digits")?;
    ops.sql(run.to, &run.db, &k2_root_proto::pg_dual::enable_subscription_sql(&n)).map_err(|e| format!("enable the subscription: {e}"))?;
    let st = status_now(dops, run).map_err(|e| format!("after enabling the subscription: {e}"))?;
    if st["target"]["enabled"].as_bool() != Some(true) {
        return Err("the subscription stays disabled after ALTER SUBSCRIPTION … ENABLE".into());
    }
    Ok(())
}

/// `--abort`, a failure, a lost slot or the 7-day limit: everything the run
/// made is dropped; the source was never paused. `failed` = the run ends
/// `failed` (a lost slot) rather than `aborted`.
pub fn abort_run(ops: &dyn ClusterOps, dops: &dyn DualOps, env: &dyn MoveEnv, store: &dyn RunStore, mut run: DualRun, why: &str, failed: bool) -> DualRun {
    let errs = undo_objects(ops, dops, &mut run);
    if !errs.is_empty() && (run.objects.source_setup || run.objects.subscribed) {
        return park(env, store, run, why, if failed { RunState::Failed } else { RunState::Aborted }, &errs);
    }
    run.finished_at = Some(env.now());
    if errs.is_empty() {
        run.state = if failed { RunState::Failed } else { RunState::Aborted };
        run.last_error = Some(why.to_string());
    } else {
        run.state = RunState::Failed;
        run.last_error = Some(format!("{why}; undo incomplete: {}", errs.join("; ")));
    }
    if let Err(e) = store.save_run(&run) {
        eprintln!("[db] live {}: run row: {e}", run.db);
    }
    env.audit("db-upgrade-live-abort", json!({"db": run.db, "run": run.id, "why": why, "state": run.state, "undo": errs}));
    note(env, 
        &format!("Database {} live move {}", run.db, run.state.as_str()),
        &format!(
            "The live move of {} to Postgres {} ended ({}): {}. The database kept serving on Postgres {} the whole time{}.",
            run.db,
            run.to,
            run.state.as_str(),
            run.last_error.clone().unwrap_or_default(),
            run.from,
            if errs.is_empty() { "; nothing of the run is left" } else { "; the doctor lists what is left (k2 db doctor)" }
        ),
    );
    run
}

/// B7: the run ended but the source's slot and freeze are still there
/// (the helper can't tear them down): open as `needs_helper`, told once,
/// retried by the watch loop. Never `failed` while they remain.
fn park(env: &dyn MoveEnv, store: &dyn RunStore, mut run: DualRun, why: &str, end_as: RunState, errs: &[String]) -> DualRun {
    run.state = RunState::NeedsHelper;
    if run.objects.end_as.is_none() {
        run.objects.end_as = Some(end_as);
        run.objects.end_why = Some(why.to_string());
    }
    run.last_error = Some(format!("{why}; waiting for the root helper to drop the replication slot and the schema-change freeze: {}", errs.join("; ")));
    let db = run.db.clone();
    tell(
        env,
        "needs_helper",
        &format!("Database {db} live move needs the root helper"),
        &format!(
            "The live move of {db} ended ({why}), but the root helper couldn't drop its replication slot and schema-change freeze ({}). \
             {db} keeps serving on Postgres {}; schema changes stay refused and the slot holds WAL until then. Update the root helper \
             (k2 doctor shows it); K2 finishes the cleanup by itself, or run k2 db upgrade {db} --abort again.",
            errs.join("; "),
            run.from
        ),
        &mut run,
    );
    if let Err(e) = store.save_run(&run) {
        eprintln!("[db] live {db}: run row: {e}");
    }
    env.audit("db-upgrade-live-needs-helper", json!({"db": db, "run": run.id, "why": why, "errors": errs}));
    run
}

// ── the watch loop (PA29, PA30) ─────────────────────────────────────────

fn tell(env: &dyn MoveEnv, episode: &str, title: &str, body: &str, run: &mut DualRun) {
    if run.lag.told.iter().any(|t| t == episode) {
        return;
    }
    run.lag.told.push(episode.to_string());
    note(env, title, body);
}

fn clear_episode(run: &mut DualRun, episode: &str) {
    run.lag.told.retain(|t| t != episode);
}

/// One look at a run (every 30 s while it is open). Never touches the
/// source's data: a lost slot or the 7-day limit end the run, never the
/// database.
pub fn tick(ops: &dyn ClusterOps, dops: &dyn DualOps, env: &dyn MoveEnv, store: &dyn RunStore, mut run: DualRun) -> DualRun {
    if run.state == RunState::NeedsHelper {
        return retry_teardown(ops, dops, env, store, run);
    }
    if !run.state.watched() {
        return run;
    }
    let loaded = run.state;
    let now = env.now();
    let age = now - run.started_at;
    let db = run.db.clone();
    // Someone (an abort, a switch) moved the run since it was read: theirs.
    let moved = |store: &dyn RunStore, run: &DualRun| store.last_run(&run.db).filter(|x| x.id == run.id).filter(|x| x.state != loaded);
    if let Some(fresh) = moved(store, &run) {
        return fresh;
    }
    if age >= RUN_MAX_SECS {
        return end_from_tick(ops, dops, env, store, run, loaded, "the 7-day limit for a live move passed", false);
    }
    for d in WARN_AT_DAYS {
        if age >= d * 86_400 {
            tell(env, &format!("day{d}"), &format!("Database {db} live move: day {d} of 7"), &format!("The live move of {db} has run {d} days; schema changes stay refused. Switch (k2 db upgrade {db} --switch) or abort; it is aborted on day 7."), &mut run);
        }
    }
    let st = match dops.dual(DualOp::Status, run.from, run.to, &db, &run.objects.object_id.clone(), None) {
        Ok(v) => v,
        Err(e) => {
            run.lag.errors = vec![format!("status: {e}")];
            run.lag.at = now;
            let _ = store.save_run_if(&run, loaded);
            return run;
        }
    };
    let (src, tgt) = (&st["source"], &st["target"]);
    let mut errors = Vec::new();
    for (side, v) in [("source", src), ("target", tgt)] {
        if let Some(e) = v["error"].as_str() {
            errors.push(format!("{side}: {e}"));
        }
    }
    let slot = &src["slot"];
    let wal_status = slot["walStatus"].as_str().map(str::to_string);
    let lag_bytes = slot["lagBytes"].as_f64().map(|f| f.max(0.0) as u64);
    let confirmed = slot["confirmedFlushLsn"].as_str().map(str::to_string);
    let progressed = confirmed != run.lag.confirmed_lsn || lag_bytes == Some(0);
    run.lag = Lag {
        at: now,
        tables_ready: tgt["tablesReady"].as_u64().unwrap_or(0),
        tables_total: tgt["tablesTotal"].as_u64().unwrap_or(0),
        lag_bytes,
        wal_status: wal_status.clone(),
        enabled: tgt["enabled"].as_bool(),
        apply_errors: tgt["applyErrors"].as_u64().unwrap_or(0),
        sync_errors: tgt["syncErrors"].as_u64().unwrap_or(0),
        confirmed_lsn: confirmed,
        last_progress_at: if progressed || run.lag.last_progress_at == 0 { now } else { run.lag.last_progress_at },
        errors,
        told: std::mem::take(&mut run.lag.told),
    };
    if wal_status.as_deref() == Some("lost") {
        return end_from_tick(
            ops,
            dops,
            env,
            store,
            run,
            loaded,
            "the replication slot passed its WAL cap and lost WAL it needed; the live database is untouched — re-run with more disk, or move by copy",
            true,
        );
    }
    if wal_status.as_deref() == Some("unreserved") {
        tell(env, "unreserved", &format!("Database {db} live move: WAL near its cap"), &format!("The live move of {db} holds WAL past the safe size (wal_status unreserved). Switch soon, or abort it; past the cap the run fails and the database is untouched."), &mut run);
    } else {
        clear_episode(&mut run, "unreserved");
    }
    let subscription_gone = tgt["subscription"].as_bool() == Some(false);
    if run.lag.enabled == Some(false) || subscription_gone {
        run.state = RunState::Stalled;
        tell(env, "stalled", &format!("Database {db} live move stalled"), &format!(
                "The live move of {db} stopped copying (apply errors {}, first-copy errors {}). See journalctl -u postgresql@{}-main on the box; then k2 db upgrade {db} --abort, fix, and run it again. The database keeps serving on Postgres {}.",
                run.lag.apply_errors, run.lag.sync_errors, run.to, run.from
            ), &mut run);
    } else if run.lag.tables_total > 0 && run.lag.tables_ready == run.lag.tables_total {
        if run.state != RunState::Streaming {
            run.state = RunState::Streaming;
            run.streaming_at.get_or_insert(now);
            tell(env, "streaming", &format!("Database {db} is ready to switch"), &format!("Every table of {db} is copied and streaming to Postgres {}. Switch when ready: k2 db upgrade {db} --switch (writes pause a few seconds).", run.to), &mut run);
        }
    } else if run.state == RunState::Streaming && run.lag.tables_ready < run.lag.tables_total {
        run.state = RunState::Syncing;
    }
    let behind = run.lag.lag_bytes.is_some_and(|b| b > LAG_WARN) || now - run.lag.last_progress_at > NO_PROGRESS_SECS;
    if behind && run.state != RunState::Stalled {
        tell(env, "lag", &format!("Database {db} live move is behind"), &format!("The live move of {db} is {} MiB behind (or made no progress for 10 minutes). It catches up by itself; if it doesn't, abort it.", run.lag.lag_bytes.unwrap_or(0) >> 20), &mut run);
    } else if !behind {
        clear_episode(&mut run, "lag");
    }
    match store.save_run_if(&run, loaded) {
        Ok(true) => run,
        // An abort or a switch won the race: its row stands.
        Ok(false) => store.last_run(&db).unwrap_or(run),
        Err(e) => {
            eprintln!("[db] live {db}: run row: {e}");
            run
        }
    }
}

/// B6: the watch loop ends a run only holding the move worker (a switch
/// or an abort holds it too), and only if nobody moved the run meanwhile.
/// Busy: nothing changes; the next tick tries again.
#[allow(clippy::too_many_arguments)]
fn end_from_tick(ops: &dyn ClusterOps, dops: &dyn DualOps, env: &dyn MoveEnv, store: &dyn RunStore, mut run: DualRun, loaded: RunState, why: &str, failed: bool) -> DualRun {
    let _held = match store.hold_worker(&run.db) {
        Ok(h) => h,
        Err(e) => {
            run.lag.errors.push(format!("ending the run waits for the move worker: {e}"));
            let _ = store.save_run_if(&run, loaded);
            return run;
        }
    };
    match store.last_run(&run.db).filter(|x| x.id == run.id) {
        Some(fresh) if fresh.state != loaded => fresh,
        _ => abort_run(ops, dops, env, store, run, why, failed),
    }
}

/// B7: a `needs_helper` run, once the helper speaks `pgdual` again: the
/// teardown and the rest of the undo, under the move worker; it ends as it
/// was going to.
fn retry_teardown(ops: &dyn ClusterOps, dops: &dyn DualOps, env: &dyn MoveEnv, store: &dyn RunStore, run: DualRun) -> DualRun {
    if dops.ready().is_err() {
        return run;
    }
    let Ok(_held) = store.hold_worker(&run.db) else { return run };
    match store.last_run(&run.db).filter(|x| x.id == run.id) {
        Some(fresh) if fresh.state != RunState::NeedsHelper => fresh,
        _ => {
            let why = run.objects.end_why.clone().unwrap_or_else(|| "the live move ended".into());
            let failed = run.objects.end_as == Some(RunState::Failed);
            abort_run(ops, dops, env, store, run, &why, failed)
        }
    }
}

// ── the switch (PA31) ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct SwitchOpts {
    pub actor: String,
    /// Owner only: skip the 60 s notice.
    pub now: bool,
    /// B4: the refresh's bound ([`refresh_timeout_secs`]).
    pub refresh_timeout_secs: Option<u64>,
}

fn status_now(dops: &dyn DualOps, run: &DualRun) -> Result<Value, String> {
    dops.dual(DualOp::Status, run.from, run.to, &run.db, &run.objects.object_id, None)
}

/// PA31 preconditions, read live: streaming, little lag, no errors.
fn ready_to_switch(dops: &dyn DualOps, env: &dyn MoveEnv, run: &DualRun) -> Result<(), String> {
    if run.state != RunState::Streaming {
        return Err(format!("not_streaming: the live move of {} is {} (it switches once every table streams)", run.db, run.state.as_str()));
    }
    let st = status_now(dops, run)?;
    let (src, tgt) = (&st["source"], &st["target"]);
    if src.get("error").is_some() || tgt.get("error").is_some() {
        return Err(format!("not_ready: {}", st));
    }
    if tgt["enabled"].as_bool() != Some(true) || tgt["applyErrors"].as_u64().unwrap_or(0) > 0 || tgt["syncErrors"].as_u64().unwrap_or(0) > 0 {
        return Err("not_ready: the subscription has errors or is disabled; --abort it".into());
    }
    let lag = src["slot"]["lagBytes"].as_f64().map(|f| f as u64).unwrap_or(u64::MAX);
    if lag >= SWITCH_MAX_LAG {
        return Err(format!("lag_too_high: {} MiB behind (it must be under 16 MiB); try again in a minute", lag >> 20));
    }
    if let Some(app) = env.rehost_running(&run.db) {
        return Err(format!("rehost_running: {app} (on this database) is being re-hosted right now; try again when it finishes"));
    }
    Ok(())
}

/// The route's synchronous refusal before it spawns the cut-over.
pub fn switch_ready(dops: &dyn DualOps, env: &dyn MoveEnv, store: &dyn RunStore, db: &str) -> Result<(), String> {
    let run = store.open_run(db).ok_or_else(|| format!("no_live_move: {db} has no live move (k2 db upgrade {db} --live starts one)"))?;
    ready_to_switch(dops, env, &run)
}

/// `--switch`: the cut-over, under the move worker (the caller holds it).
/// `Ok` is the cut-over's own move row (method `dual`): succeeded,
/// aborted (undone; the run streams on or was dropped), rolled back or
/// failed. `Err` = refused before anything changed.
pub fn switch(ops: &dyn ClusterOps, dops: &dyn DualOps, env: &dyn MoveEnv, store: &dyn RunStore, db: &str, o: &SwitchOpts) -> Result<(MoveProgress, DualRun), String> {
    let mut dr = store.open_run(db).ok_or_else(|| format!("no_live_move: {db} has no live move (k2 db upgrade {db} --live starts one)"))?;
    ready_to_switch(dops, env, &dr)?;
    let rec = env.record(db)?;
    if rec.major != dr.from {
        return Err(format!("inconsistent_record: {db} is on Postgres {}, the run started from {}", rec.major, dr.from));
    }
    let rows = ops.clusters()?;
    let mut p = new_progress("move", db, dr.from, dr.to, &o.actor, env.now());
    p.method = "dual".into();
    p.from_port = super::port_of(&rows, dr.from);
    p.to_port = super::port_of(&rows, dr.to).or(dr.objects.target_port);
    dr.move_id = Some(p.id.clone());
    dr.state = RunState::Switching;
    save(store, &dr)?;
    let mut run = Run { ops, env, p };
    if let Err(StepFail(e)) = run.save() {
        dr.state = RunState::Streaming;
        save(store, &dr)?;
        return Err(format!("the switch can't be recorded: {e}"));
    }
    let res = switch_steps(&mut run, dops, store, &mut dr, o.now, refresh_timeout_secs(o.refresh_timeout_secs));
    let p = settle_switch(run, dops, store, &mut dr, res);
    Ok((p, dr))
}

/// The end of a cut-over, success or not, for the run row too.
fn settle_switch(run: Run, dops: &dyn DualOps, store: &dyn RunStore, dr: &mut DualRun, res: Result<(), StepFail>) -> MoveProgress {
    let (ops, env) = (run.ops, run.env);
    let detached = dr.objects.detached;
    let p = run.settle(res);
    match p.state {
        MoveState::Succeeded => {
            dr.state = RunState::Switched;
            dr.finished_at = Some(env.now());
        }
        MoveState::Aborted | MoveState::RolledBack if !detached => {
            // Undone before the stream was detached: the run streams on,
            // once its subscription is enabled again; else it ends failed.
            dr.state = RunState::Streaming;
            dr.last_error = p.last_error.clone();
            if let Err(e) = resume_stream(ops, dops, dr) {
                if let Err(e2) = store.save_run(dr) {
                    eprintln!("[db] live {}: run row: {e2}", dr.db);
                }
                let why = format!("{}; then the stream couldn't resume: {e}", p.last_error.clone().unwrap_or_default());
                *dr = abort_run(ops, dops, env, store, dr.clone(), &why, true);
                return p;
            }
        }
        MoveState::Aborted | MoveState::RolledBack => {
            // The target copy was dropped by the undo; what is left of the
            // run (the role, maybe the slot) goes too.
            dr.objects.target_created = false;
            let mut left = dr.clone();
            let errs = undo_objects(ops, dops, &mut left);
            if !errs.is_empty() && (left.objects.source_setup || left.objects.subscribed) {
                *dr = park(env, store, left, &p.last_error.clone().unwrap_or_default(), RunState::Aborted, &errs);
                return p;
            }
            *dr = left;
            dr.state = if errs.is_empty() { RunState::Aborted } else { RunState::Failed };
            dr.finished_at = Some(env.now());
            dr.last_error = Some(match errs.is_empty() {
                true => p.last_error.clone().unwrap_or_default(),
                false => format!("{}; undo incomplete: {}", p.last_error.clone().unwrap_or_default(), errs.join("; ")),
            });
        }
        _ => {
            dr.state = RunState::Failed;
            dr.finished_at = Some(env.now());
            dr.last_error = p.last_error.clone();
        }
    }
    if let Err(e) = store.save_run(dr) {
        eprintln!("[db] live {}: run row: {e}", dr.db);
    }
    p
}

fn switch_steps(run: &mut Run, dops: &dyn DualOps, store: &dyn RunStore, dr: &mut DualRun, now: bool, refresh_secs: u64) -> Result<(), StepFail> {
    let (db, from, to) = (dr.db.clone(), dr.from, dr.to);
    let from_port = run.p.from_port.ok_or("the source cluster has no port on record")?;
    let to_port = run.p.to_port.ok_or("the target cluster has no port on record")?;
    let id = dr.objects.object_id.clone();
    // 1. notice
    run.p.doors_paused = true;
    run.at(Step::Notice, MoveState::Checking)?;
    run.env.notice(&db, &format!("Database {db} switches to Postgres {to}: writes pause for a few seconds in {} s.", if now { 0 } else { NOTICE_SECS }));
    run.env.pause_doors(&db, true);
    if !now {
        run.env.sleep_secs(NOTICE_SECS);
    }
    // 2. the write block: the walsender keeps streaming (L6). Apps that
    // can't run two copies stop here; the others stay up until step 8.
    let users = run.env.users(&db);
    let one_copy = run.env.one_copy_apps(&users.apps);
    run.p.stopped_apps = one_copy.clone();
    run.at(Step::Quiesce, MoveState::Copying)?;
    run.p.stopped_apps = run.env.stop_apps(&one_copy).map_err(|e| format!("stop Apps: {e}"))?;
    run.p.quiesced = true;
    run.save()?;
    run.ops.allow_conn(from, &db, ConnMode::QuiesceRepl).map_err(|e| format!("write block: {e}"))?;
    let blocked_at = run.env.now();
    // 3. drain: the slot catches up to the source's WAL position (≤ 60 s).
    // Security review B2: the write block returned only once every other
    // session had exited, so every commit is in WAL now; the target is the
    // INSERT position (a `synchronous_commit = off` commit can sit past
    // the write position), read after the block.
    run.at(Step::Copy, MoveState::Copying)?;
    let st = status_now(dops, dr).map_err(|e| format!("drain: {e}"))?;
    let target = st["source"]["insertLsn"].as_str().and_then(catalog::lsn).ok_or("drain: the source answered no WAL insert position")?;
    let mut drained = false;
    for _ in 0..=DRAIN_WAIT_SECS {
        let st = status_now(dops, dr).map_err(|e| format!("drain: {e}"))?;
        let slot = &st["source"]["slot"];
        if st["source"]["walsenders"].as_u64().unwrap_or(0) == 0 || slot["active"].as_bool() != Some(true) {
            return Err("drain: the replication connection dropped during the write block; the switch is undone and the run streams on (try --switch again)".into());
        }
        if slot["confirmedFlushLsn"].as_str().and_then(catalog::lsn).is_some_and(|c| c >= target) {
            drained = true;
            break;
        }
        run.env.sleep_secs(1);
    }
    if !drained {
        return Err("drain: the copy didn't catch up within 60 s; the switch is undone and the run streams on (try --switch again)".into());
    }
    // Security review B3: logical replication never copies large objects,
    // and `lo_create` isn't DDL (the freeze lets it through). Looked at
    // again under the write block: any at all, and the cut-over is undone.
    let lobs = run.ops.sql(from, &db, LARGE_OBJECTS_SQL).map_err(|e| format!("large objects: {e}"))?;
    match lobs.trim().parse::<u64>() {
        Ok(0) => {}
        Ok(n) => {
            return Err(format!(
                "large_objects: {db} gained {n} large object(s) during the live move; logical replication can't copy them. The switch is undone; abort (k2 db upgrade {db} --abort) and move by copy"
            )
            .into())
        }
        Err(_) => return Err(format!("large objects: unexpected answer {:?}", lobs.trim()).into()),
    }
    // 4. detach (L24). From here an undo drops the target copy.
    dops.dual(DualOp::Detach, from, to, &db, &id, None).map_err(|e| format!("detach: {e}"))?;
    dr.objects.detached = true;
    dr.objects.subscribed = false;
    run.p.target_created = true;
    run.save()?;
    save(store, dr).map_err(StepFail)?;
    // Security review B1: the target was quiesced (CONNECTION LIMIT 0)
    // since setup; the owner's refresh (6) and the logins (7) need it open.
    // No App reaches it before the re-point (8); an undo quiesces it again
    // before the drop.
    run.ops.allow_conn(to, &db, ConnMode::On).map_err(|e| format!("open the new copy to K2's roles: {e}"))?;
    // 5. sequences, exact (L21).
    let seqs = run.ops.sql(from, &db, catalog::sequences_sql())?;
    let script = catalog::setval_script(&seqs).ok_or("sequences: unexpected answer")?;
    if !script.is_empty() {
        run.ops.sql(to, &db, &script).map_err(|e| format!("sequences: {e}"))?;
    }
    // 6. FORCE back; materialized views refreshed as the owner.
    dops.force_rls(to, &db, &dr.objects.forced_oids).map_err(|e| format!("force-rls: {e}"))?;
    let props: Value = serde_json::from_str(&run.ops.sql(from, &db, catalog::db_props_sql())?).map_err(|_| "database properties are not JSON")?;
    let owner = props["owner"].as_str().ok_or("the database has no owner")?.to_string();
    let logins = run.env.logins(&db)?;
    let owner_pw = logins.iter().find(|(r, _)| *r == owner).map(|(_, p)| p.clone()).ok_or("the database's owner is not K2's migrator")?;
    dops.refresh_matviews(to, to_port, &db, &owner, &owner_pw, refresh_secs).map_err(|e| format!("refresh materialized views: {e}"))?;
    // 7. verify: every row counted, the catalog fingerprint, sequences.
    run.at(Step::Verify, MoveState::Verifying)?;
    let counts_src = row_counts(run.ops, from, &db)?;
    let counts_dst = row_counts(run.ops, to, &db)?;
    let mut print_src = fingerprint(run.ops, from, &db)?;
    let mut print_dst = fingerprint(run.ops, to, &db)?;
    catalog::normalize_privileges(&mut print_src, from);
    catalog::normalize_privileges(&mut print_dst, from);
    let mut diff = catalog::diff(&json!({"rows": counts_src}), &json!({"rows": counts_dst}));
    diff.extend(catalog::diff(&print_src, &print_dst));
    for (role, pw) in &logins {
        if let Err(e) = run.ops.login(to, to_port, &db, role, pw) {
            diff.push(format!("login {role}: {e}"));
        }
    }
    if !diff.is_empty() {
        run.p.diff = diff.clone();
        return Err(format!("verify: {} difference(s), first: {}", diff.len(), diff[0]).into());
    }
    // 8. switch: the record, the vault, then every App re-hosted.
    run.at(Step::Switch, MoveState::Switching)?;
    run.env.set_major(&db, to).map_err(|e| format!("switch the record: {e}"))?;
    run.p.switched = true;
    run.p.target_created = false;
    run.save()?;
    match run.env.repoint(&db, from_port, to_port) {
        Ok(done) => {
            for d in &done {
                if let Some(w) = d.strip_prefix("skipped: ") {
                    run.p.warnings.push(format!("not re-pointed: {w}"));
                }
            }
            run.p.repointed = done;
        }
        Err((done, why)) => {
            let back = run.env.set_major(&db, from).and_then(|_| run.env.unpoint(&db, &done, from_port, to_port));
            return match back {
                Ok(()) => {
                    run.p.switched = false;
                    run.p.target_created = true;
                    let errs = run.undo_before_switch();
                    let msg = format!("re-point: {why}");
                    if errs.is_empty() {
                        run.env.audit("db-upgrade-live-switch-rolled-back", json!({"db": db, "why": msg}));
                        run.finish(MoveState::RolledBack, Some(msg));
                    } else {
                        run.fail(format!("{msg}; then: {}", errs.join("; ")));
                    }
                    Err(StepFail::done())
                }
                Err(e) => {
                    run.p.repointed = done;
                    run.fail(format!("re-point: {why}; reversing it failed: {e}"));
                    Err(StepFail::done())
                }
            };
        }
    }
    finish_forward(run, dops, dr, from, to, to_port, Some(blocked_at), &users.apps)
}

/// Steps 9–10 (also the recovery path at or after step 8): no undo from
/// here (F4).
fn finish_forward(run: &mut Run, dops: &dyn DualOps, dr: &mut DualRun, from: Major, to: Major, to_port: u16, blocked_at: Option<i64>, apps: &[String]) -> Result<(), StepFail> {
    let db = dr.db.clone();
    // PA10: every App on the database re-hosted on the new DSN (running
    // ones get a fresh instance; the stopped ones start).
    let mut all: Vec<String> = apps.to_vec();
    all.extend(run.p.stopped_apps.iter().cloned());
    all.sort();
    all.dedup();
    let failed = run.env.rehost_apps(&all, &run.p.id);
    if !failed.is_empty() {
        run.p.warnings.push(format!("Apps that didn't go live on Postgres {to}: {}", failed.join(", ")));
    }
    run.p.stopped_apps.clear();
    // 9. resume
    run.at(Step::Resume, MoveState::Switching)?;
    if let Err(e) = run.ops.allow_conn(to, &db, ConnMode::On) {
        return Err(format!("open the new copy: {e}").into());
    }
    run.env.pause_doors(&db, false);
    run.p.doors_paused = false;
    run.env.clear_notice(&db);
    let paused = blocked_at.map(|b| (run.env.now() - b).max(0));
    if let Err(e) = run.ops.sql(to, &db, "ANALYZE;\n") {
        run.p.warnings.push(format!("ANALYZE on {to}: {e}"));
    }
    if let Err(e) = run.env.rerun_k2_setup(to, &db) {
        run.p.warnings.push(format!("K2 setup on {to}: {e}"));
    }
    if let Err(e) = run.env.verify_k2(to, &db) {
        run.p.warnings.push(format!("K2 check through the guest door: {e}"));
    }
    // 10. record: the replication role goes while the source is still
    // reachable by the helper (security review A-M1: closed first, the
    // teardown couldn't get in and the role stayed), then the source
    // stays, closed.
    run.at(Step::Record, MoveState::Switching)?;
    match dops.dual(DualOp::Teardown, from, to, &db, &dr.objects.object_id, None) {
        Ok(_) => {
            dr.objects.source_setup = false;
        }
        Err(e) => run.p.warnings.push(format!("drop the replication role: {e} (the doctor lists it; k2 db doctor)")),
    }
    if let Err(e) = run.ops.allow_conn(from, &db, ConnMode::Off) {
        run.p.warnings.push(format!("close the kept source: {e}"));
    }
    run.p.quiesced = false;
    let now = run.env.now();
    let info = MovedInfo { from, at: now, until: now + ROLLBACK_WINDOW_SECS, tag: run.p.tag.clone(), source_state: SourceState::Kept };
    run.env.set_moved(&db, Some(&info)).map_err(|e| format!("record the move: {e}"))?;
    run.env.audit(
        "db-upgrade-live-switch",
        json!({"db": db, "from": from, "to": to, "port": to_port, "actor": run.p.actor, "run": dr.id, "pausedSecs": paused, "warnings": run.p.warnings}),
    );
    note(run.env, 
        &format!("Database {db} moved {from} → {to}"),
        &format!(
            "Database {db} moved {from} → {to} live by {}; writes paused for {} s. The {from} copy is kept (closed) for 14 days: \
             `k2 db upgrade {db} --rollback --yes` (writes made on {to} since the switch are not carried back).",
            run.p.actor,
            paused.map(|s| s.to_string()).unwrap_or_else(|| "a few".into())
        ),
    );
    run.p.step = Step::Done;
    run.finish(MoveState::Succeeded, None);
    Ok(())
}

/// B3: the large objects of the database (none may exist at the switch).
pub const LARGE_OBJECTS_SQL: &str = "SELECT count(*) FROM pg_catalog.pg_largeobject_metadata;\n";

// ── daemon restarts (PA36) and orphans (PA35) ───────────────────────────

/// A cut-over the last daemon died in (its `sql_moves` row, method
/// `dual`): before the record points at the target, undo (the source
/// re-opened; the run streams on when its subscription is still there,
/// else it is dropped); at or after it, finish forward.
pub fn recover_switch(ops: &dyn ClusterOps, dops: &dyn DualOps, env: &dyn MoveEnv, store: &dyn RunStore, p: MoveProgress) -> MoveProgress {
    let db = p.db.clone();
    let mut run = Run { ops, env, p };
    run.p.state = MoveState::Interrupted;
    let _ = env.progress(&run.p);
    let Some(mut dr) = store.open_run(&db).or_else(|| store.last_run(&db)) else {
        return run.fail("interrupted live switch with no run row".into());
    };
    let rec = match env.record(&db) {
        Ok(r) => r,
        Err(e) => return run.fail(format!("interrupted; the record can't be read: {e}")),
    };
    let switched = run.p.switched || rec.major == run.p.to;
    if switched {
        run.p.switched = true;
        run.p.target_created = false;
        let (Some(from_port), Some(to_port)) = (run.p.from_port, run.p.to_port) else {
            return run.fail("interrupted after the switch with no ports on record".into());
        };
        if run.p.step <= Step::Switch {
            match env.repoint(&db, from_port, to_port) {
                Ok(done) => run.p.repointed = done,
                Err((_, why)) => return run.fail(format!("recover: re-point: {why}")),
            }
        }
        let apps = env.users(&db).apps;
        let (from, to) = (run.p.from, run.p.to);
        let res = finish_forward(&mut run, dops, &mut dr, from, to, to_port, None, &apps);
        return settle_switch(run, dops, store, &mut dr, res);
    }
    // Before the switch: the target is dropped only once detached.
    let mut why = format!("interrupted at {:?} by a daemon restart", run.p.step);
    if !dr.objects.detached {
        // Still subscribed? Then it streams on. Gone (the target says so)?
        // Drop what is left. Unknown (status unreadable)? Never a guess
        // that drops anything (security review B5): it streams on, the
        // watch loop looks again and stalls it if the stream is gone.
        match status_now(dops, &dr) {
            Ok(s) if s["target"]["subscription"].as_bool() == Some(false) => dr.objects.detached = true,
            Ok(_) => {}
            Err(e) => why = format!("{why}; the stream's state can't be read ({e}), so nothing was dropped"),
        }
    }
    // After the look (B5): a detached run's undo drops the target copy.
    run.p.target_created = dr.objects.detached;
    let res: Result<(), StepFail> = Err(StepFail(why));
    settle_switch(run, dops, store, &mut dr, res)
}

/// Once per boot (PA36): a run left setting up is dropped; a cut-over
/// whose move row already ended gets the run row to match (an open move
/// row is the move recovery's).
pub fn boot_runs(ops: &dyn ClusterOps, dops: &dyn DualOps, env: &dyn MoveEnv, store: &dyn RunStore) -> Vec<String> {
    let mut out = Vec::new();
    for run in store.open_runs() {
        match run.state {
            RunState::SettingUp => {
                let db = run.db.clone();
                let r = abort_run(ops, dops, env, store, run, "interrupted during setup by a daemon restart", false);
                out.push(format!("{db}: {}", r.state.as_str()));
            }
            RunState::Switching => {
                let ms = run.move_id.as_deref().and_then(|m| store.move_state(m));
                let mut r = run.clone();
                match ms {
                    Some(MoveState::Succeeded) => {
                        r.state = RunState::Switched;
                        r.finished_at = Some(env.now());
                    }
                    Some(MoveState::Failed) => r.state = RunState::Failed,
                    None | Some(MoveState::Aborted | MoveState::RolledBack) => {
                        // Undone (or never recorded): the source is opened
                        // again, the run streams on.
                        let _ = ops.allow_conn(r.from, &r.db, ConnMode::On);
                        r.state = RunState::Streaming;
                    }
                    Some(_) => continue,
                }
                let _ = store.save_run(&r);
                out.push(format!("{}: {}", r.db, r.state.as_str()));
            }
            _ => {}
        }
    }
    out
}

/// PA35: every `k2_dual_*` slot and `k2_repl_*` role on the online
/// clusters; those whose run is over are torn down (idempotent). An
/// inactive slot K2 can't clean is a doctor failure.
pub fn sweep_orphans(ops: &dyn ClusterOps, dops: &dyn DualOps, store: &dyn RunStore, clean: bool) -> Value {
    let open: Vec<String> = store.open_runs().into_iter().map(|r| r.objects.object_id).collect();
    let mut found = Vec::new();
    let mut cleaned = Vec::new();
    let mut failures = Vec::new();
    let Ok(rows) = ops.clusters() else { return json!({"error": "clusters unreadable"}) };
    for c in rows.iter().filter(|r| r.name == "main" && r.online()) {
        let Ok(t) = ops.sql(c.major, "postgres", k2_root_proto::pg_dual::ORPHANS_SQL) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(t.trim()) else { continue };
        let mut ids: Vec<(String, Option<String>, Option<bool>)> = Vec::new();
        for s in v["slots"].as_array().cloned().unwrap_or_default() {
            if let Some(id) = s["name"].as_str().and_then(|n| n.strip_prefix(k2_root_proto::pg_dual::OBJECT_PREFIX)) {
                ids.push((id.to_string(), s["db"].as_str().map(str::to_string), s["active"].as_bool()));
            }
        }
        for r in v["roles"].as_array().cloned().unwrap_or_default() {
            if let Some(id) = r.as_str().and_then(|n| n.strip_prefix(k2_root_proto::pg_dual::ROLE_PREFIX)) {
                if !ids.iter().any(|(i, _, _)| i == id) {
                    ids.push((id.to_string(), None, None));
                }
            }
        }
        for (id, slot_db, active) in ids {
            if open.contains(&id) || !k2_root_proto::pg_dual::dual_id_ok(&id) {
                continue;
            }
            let run = store.run_by_object(&id);
            found.push(json!({"major": c.major, "id": id, "db": slot_db, "active": active, "run": run.as_ref().map(|r| r.id.clone())}));
            match run {
                Some(r) if clean && !r.state.open() => {
                    let db = slot_db.unwrap_or_else(|| r.db.clone());
                    match dops.dual(DualOp::Teardown, r.from, r.to, &db, &id, None) {
                        Ok(_) => cleaned.push(format!("{}/{id}", c.major)),
                        Err(e) => failures.push(format!("{}/{id}: {e}", c.major)),
                    }
                }
                _ if active == Some(false) => failures.push(format!("{}/k2_dual_{id}: an inactive slot holding WAL that K2 can't tie to a run", c.major)),
                _ => {}
            }
        }
    }
    json!({"orphans": found, "cleaned": cleaned, "failures": failures})
}

/// The `--prepare-live <major>` steps (PA21, owner only): the drop-in and
/// guard, then (only when the running cluster isn't logical yet) a notice
/// and one restart. Answers what happened.
pub fn prepare_live(ops: &dyn ClusterOps, dops: &dyn DualOps, env: &dyn MoveEnv, m: Major, now: bool, notice: &dyn Fn(Major, &str)) -> Result<Value, String> {
    let free = ops.free_bytes()?;
    let v = dops.logical_conf(m, Some(wal_keep_mb(free)), false)?;
    if v["walLevel"].as_str() == Some("logical") {
        return Ok(json!({"major": m, "restarted": false, "walLevel": "logical", "note": "already logical; nothing restarted"}));
    }
    notice(m, &format!("Postgres {m} restarts in {} s to allow live moves: every database on it pauses for about 5-15 s.", if now { 0 } else { NOTICE_SECS }));
    if !now {
        env.sleep_secs(NOTICE_SECS);
    }
    dops.restart_cluster(m)?;
    let after = ops.sql(m, "postgres", "SHOW wal_level;\n")?;
    if after.trim() != "logical" {
        return Err(format!("restarted, but Postgres {m} still runs with wal_level = {}; see k2 db doctor", after.trim()));
    }
    env.audit("db-upgrade-prepare-live", json!({"major": m, "walLevel": "logical"}));
    Ok(json!({"major": m, "restarted": true, "walLevel": "logical"}))
}
