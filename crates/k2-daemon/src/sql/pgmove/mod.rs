//! `k2 db upgrade`: move one database to a newer Postgres major by copy
//! (`prd-pg17-and-db-upgrade-v1` rev 2 §6, S5).
//!
//! The machine runs over two seams so it is testable without Postgres and
//! independent of where K2 keeps its records:
//! - [`ClusterOps`]: the clusters (the root helper's protocol-9 `pg`
//!   verbs in production, [`helper_ops::HelperOps`]);
//! - [`MoveEnv`]: K2's side: the per-database record and the resolver
//!   (migration 0201, PG-A's S1), Apps, guest doors, the vault re-point,
//!   notices, audit, inbox, and the move's own progress row.
//!
//! The steps (§6.2): check → target → notice → roles → quiesce → copy →
//! verify → switch → resume → record. Failures follow §13: before the
//! switch everything is undone and the source keeps serving (`aborted`);
//! a failed re-point is reversed (`rolled_back`); a failed reversal
//! latches `failed`. `--rollback` (§6.3) and `--forget-old` (§6.4) are
//! [`rollback`] and [`forget_old`]; a move interrupted by a daemon restart
//! is finished or undone by [`recover`].
//!
//! `k2 db restore` is never used here: it restores as the non-superuser
//! migrator with `--clean` and carries no roles (V43). The copy is the
//! helper's `move-dump`/`move-restore`, as postgres, with the target
//! major's binaries.

pub mod catalog;
pub mod check;
pub mod helper_ops;
pub mod request;
pub mod worker;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod real_move;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use k2_root_proto::pg_major::{in_band, role_name_ok, ClusterRow, BAND};
pub use k2_root_proto::verbs::ConnMode;

pub type Major = u32;

/// §6.3: the source is kept this long after a move.
pub const ROLLBACK_WINDOW_SECS: i64 = 14 * 86_400;
/// §6.4: forget-old waits this long after a move (unless `--force-now`).
pub const FORGET_MIN_AGE_SECS: i64 = 86_400;
/// §6.2 step 3: the notice before an agent's move (owner: `--now`).
pub const NOTICE_SECS: u64 = 60;

// ── seams ───────────────────────────────────────────────────────────────

/// One `pg allow-conn`-style lock state of a database.
pub trait ClusterOps {
    fn clusters(&self) -> Result<Vec<ClusterRow>, String>;
    fn installed(&self) -> Result<Vec<Major>, String>;
    /// Ports in K2's band something already listens on.
    fn band_listening(&self) -> Result<Vec<u16>, String>;
    fn add_major(&self, m: Major) -> Result<(), String>;
    fn ensure_cluster(&self, m: Major, port: u16) -> Result<(), String>;
    /// SQL as postgres in `db` on `m`'s cluster (`-tA`, stop on error).
    fn sql(&self, m: Major, db: &str, sql: &str) -> Result<String, String>;
    fn copy_roles(&self, from: Major, to: Major, roles: &[String], dry_run: bool) -> Result<Value, String>;
    fn allow_conn(&self, m: Major, db: &str, mode: ConnMode) -> Result<(), String>;
    /// Bytes of the dump.
    fn dump(&self, from: Major, to: Major, db: &str, tag: &str) -> Result<u64, String>;
    fn restore(&self, to: Major, db: &str, tag: &str) -> Result<(), String>;
    fn drop_db(&self, m: Major, db: &str) -> Result<bool, String>;
    fn remove_dump(&self, db: &str, tag: &str) -> Result<(), String>;
    /// Free bytes on the clusters' data filesystem.
    fn free_bytes(&self) -> Result<u64, String>;
    /// `None`: that major isn't installed (yet).
    fn extension_available(&self, m: Major, ext: &str) -> Option<bool>;
    /// A role logs in over TCP on `port` with its vaulted password.
    fn login(&self, m: Major, port: u16, db: &str, role: &str, password: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterRole {
    K2,
    Adopted,
    Foreign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    #[default]
    None,
    Kept,
    Forgotten,
}

/// K2's record of one database (`sql_databases` + 0201's columns).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbRecord {
    pub name: String,
    pub project_id: String,
    pub major: Major,
    /// K2's roles for it: `{db}_migrator`, `{db}_agent`, the bind role,
    /// every grantee login from `sql_grants`.
    pub k2_roles: Vec<String>,
    pub moved_from: Option<Major>,
    pub moved_at: Option<i64>,
    pub rollback_until: Option<i64>,
    pub source_state: SourceState,
    /// The finished move's tag (its dump's name).
    pub move_tag: Option<String>,
}

/// Who uses the database (§6.1 "Users", V35–V37).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Users {
    /// Apps holding a vaulted `DATABASE_URL` for it (re-pointed, restarted).
    pub apps: Vec<String>,
    /// Literal DSNs K2 found but can't rewrite (the owner fixes them).
    pub literal_dsns: Vec<String>,
    pub guest_doors: u32,
}

/// The record a finished move writes (§6.2 step 10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovedInfo {
    pub from: Major,
    pub at: i64,
    pub until: i64,
    pub tag: String,
    pub source_state: SourceState,
}

/// K2's side of a move.
pub trait MoveEnv {
    fn now(&self) -> i64;
    fn sleep_secs(&self, s: u64);
    fn record(&self, db: &str) -> Result<DbRecord, String>;
    fn cluster_role(&self, m: Major) -> ClusterRole;
    /// (role, password) pairs to prove after the copy (`{db}_agent`, `{db}_migrator`).
    fn logins(&self, db: &str) -> Result<Vec<(String, String)>, String>;
    fn users(&self, db: &str) -> Users;
    fn notice(&self, db: &str, text: &str);
    fn clear_notice(&self, db: &str);
    /// Guest doors for the database answer `503 db_maintenance`.
    fn pause_doors(&self, db: &str, paused: bool);
    /// Stop these Apps; `Ok` lists the ones that were running (to start again).
    fn stop_apps(&self, apps: &[String]) -> Result<Vec<String>, String>;
    /// Start them; answers the ones that didn't start.
    fn start_apps(&self, apps: &[String]) -> Vec<String>;
    /// Re-run K2's idempotent per-database setup on `m` (helpers, the
    /// `_k2_*` seal, privilege sync) through the resolver (V34).
    fn rerun_k2_setup(&self, m: Major, db: &str) -> Result<(), String>;
    /// Extra checks through K2 itself (guest door rows/query) once the
    /// record points at `m`.
    fn verify_k2(&self, m: Major, db: &str) -> Result<(), String>;
    /// Re-point every user (vaulted DSNs, …) from one port to the other;
    /// `Err((done, why))` when it stopped midway.
    fn repoint(&self, db: &str, from_port: u16, to_port: u16) -> Result<Vec<String>, (Vec<String>, String)>;
    /// Reverse the given re-point steps (newest first).
    fn unpoint(&self, db: &str, done: &[String], from_port: u16, to_port: u16) -> Result<(), String>;
    fn set_major(&self, db: &str, m: Major) -> Result<(), String>;
    fn set_moved(&self, db: &str, info: Option<&MovedInfo>) -> Result<(), String>;
    fn record_cluster(&self, m: Major, port: u16);
    /// Persist the move's row (`sql_moves`); called at every step.
    fn progress(&self, p: &MoveProgress);
    fn audit(&self, act: &str, line: Value);
    fn inbox(&self, title: &str, body: &str);
}

// ── progress ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Check,
    Target,
    Notice,
    Roles,
    Quiesce,
    Copy,
    Verify,
    Switch,
    Resume,
    Record,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveState {
    Checking,
    Copying,
    Verifying,
    Switching,
    Succeeded,
    Aborted,
    RolledBack,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveProgress {
    pub id: String,
    pub db: String,
    pub from: Major,
    pub to: Major,
    pub tag: String,
    pub actor: String,
    pub state: MoveState,
    pub step: Step,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub last_error: Option<String>,
    pub from_port: Option<u16>,
    pub to_port: Option<u16>,
    /// The target database was created (undo drops it).
    pub target_created: bool,
    /// The source was quiesced (undo re-opens it).
    pub quiesced: bool,
    pub stopped_apps: Vec<String>,
    pub doors_paused: bool,
    pub repointed: Vec<String>,
    pub roles_created: Vec<String>,
    pub dump_bytes: Option<u64>,
    /// Verify's differences (counts, ACLs, policies…) when it failed.
    pub diff: Vec<String>,
    pub warnings: Vec<String>,
    pub check: Vec<check::CheckLine>,
}

/// What the caller may skip or widen (§12: owner-only flags).
#[derive(Debug, Clone, Default)]
pub struct MoveOpts {
    pub to: Option<Major>,
    pub actor: String,
    /// Owner only: skip the 60 s notice.
    pub now: bool,
    /// Owner only: move out of an adopted cluster.
    pub allow_adopted: bool,
    /// Helper protocol ≥ 9 with `namespaces.pgmajor`.
    pub helper_ok: bool,
}

/// `YYYYMMDDTHHMMSSZ` for a unix time.
pub fn tag_for(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, (rem / 60) % 60, rem % 60)
}

pub(crate) fn port_of(rows: &[ClusterRow], m: Major) -> Option<u16> {
    rows.iter().find(|r| r.major == m && r.name == "main").map(|r| r.port)
}

/// The first free port in K2's band: not a cluster's, not listening.
pub fn free_band_port(rows: &[ClusterRow], listening: &[u16]) -> Option<u16> {
    (BAND.0..=BAND.1).find(|p| !rows.iter().any(|r| r.port == *p) && !listening.contains(p))
}

fn ok_name(n: &str) -> bool {
    role_name_ok(n)
}

/// Every role the move recreates on the target (§6.1 Roles, V32–V33).
pub fn roles_to_copy(ops: &dyn ClusterOps, from: Major, rec: &DbRecord) -> Result<Vec<String>, String> {
    let t = ops.sql(from, &rec.name, &catalog::role_set_sql(&rec.k2_roles))?;
    let mut v: Vec<String> = serde_json::from_str(&t).map_err(|_| "role set answer is not JSON".to_string())?;
    v.sort();
    v.dedup();
    if let Some(bad) = v.iter().find(|r| !ok_name(r)) {
        return Err(format!(
            "role_name_unsupported: the database uses a role K2 can't carry ({} chars, not [a-z0-9_])",
            bad.len()
        ));
    }
    Ok(v)
}

/// Row counts per table, one JSON object.
pub fn row_counts(ops: &dyn ClusterOps, m: Major, db: &str) -> Result<Value, String> {
    let union = ops.sql(m, db, &catalog::count_query_sql())?;
    let wrapped = catalog::count_wrap(&union).ok_or("count query refused")?;
    let t = ops.sql(m, db, &wrapped)?;
    serde_json::from_str(&t).map_err(|_| "row counts are not JSON".into())
}

pub fn fingerprint(ops: &dyn ClusterOps, m: Major, db: &str) -> Result<Value, String> {
    let t = ops.sql(m, db, &catalog::fingerprint_sql())?;
    serde_json::from_str(&t).map_err(|_| "fingerprint is not JSON".into())
}

// ── the move ────────────────────────────────────────────────────────────

/// Why a step failed, and how far the undo must go.
struct StepFail(String);

impl<T: Into<String>> From<T> for StepFail {
    fn from(s: T) -> Self {
        StepFail(s.into())
    }
}

struct Run<'a> {
    ops: &'a dyn ClusterOps,
    env: &'a dyn MoveEnv,
    p: MoveProgress,
}

impl Run<'_> {
    fn at(&mut self, step: Step, state: MoveState) {
        self.p.step = step;
        self.p.state = state;
        self.env.progress(&self.p);
    }

    fn finish(&mut self, state: MoveState, err: Option<String>) -> MoveProgress {
        self.p.state = state;
        self.p.last_error = err;
        self.p.finished_at = Some(self.env.now());
        self.env.progress(&self.p);
        self.p.clone()
    }

    /// Undo everything before the switch (§13 rows 1–7): drop the target
    /// copy, re-open the source, start the Apps, open the doors, clear the
    /// notice. The dump stays with the record.
    fn undo_before_switch(&mut self) -> Vec<String> {
        let mut errs = Vec::new();
        let (db, from, to) = (self.p.db.clone(), self.p.from, self.p.to);
        if self.p.target_created {
            match self.ops.drop_db(to, &db) {
                Ok(_) => self.p.target_created = false,
                Err(e) => errs.push(format!("drop the target copy: {e}")),
            }
        }
        if self.p.quiesced {
            match self.ops.allow_conn(from, &db, ConnMode::On) {
                Ok(()) => self.p.quiesced = false,
                Err(e) => errs.push(format!("re-open the source: {e}")),
            }
        }
        if !self.p.stopped_apps.is_empty() {
            let failed = self.env.start_apps(&self.p.stopped_apps);
            if !failed.is_empty() {
                errs.push(format!("Apps that didn't start again: {}", failed.join(", ")));
            }
            self.p.stopped_apps.clear();
        }
        if self.p.doors_paused {
            self.env.pause_doors(&db, false);
            self.p.doors_paused = false;
        }
        self.env.clear_notice(&db);
        errs
    }

    fn abort(&mut self, why: String) -> MoveProgress {
        let errs = self.undo_before_switch();
        if errs.is_empty() {
            self.env.audit("db-upgrade-aborted", json!({"db": self.p.db, "from": self.p.from, "to": self.p.to, "why": why}));
            self.finish(MoveState::Aborted, Some(why))
        } else {
            // The undo itself failed: new moves wait for the owner.
            let msg = format!("{why}; undo incomplete: {}", errs.join("; "));
            self.env.audit("db-upgrade-failed", json!({"db": self.p.db, "why": msg}));
            self.env.inbox(
                &format!("Database {} move failed", self.p.db),
                &format!("The move {} → {} stopped and could not be fully undone: {msg}. New moves are blocked until the owner runs `k2 db upgrade --acknowledge-failed`.", self.p.from, self.p.to),
            );
            self.finish(MoveState::Failed, Some(msg))
        }
    }
}

/// `--check` alone: the report, nothing changed.
pub fn check_only(ops: &dyn ClusterOps, env: &dyn MoveEnv, db: &str, opts: &MoveOpts) -> Result<check::Report, String> {
    let rec = env.record(db)?;
    check::run(ops, env, &rec, opts)
}

/// One move, start to finish (§6.2). Never panics on a Postgres or K2
/// failure: the answer's state says what happened.
pub fn run_move(ops: &dyn ClusterOps, env: &dyn MoveEnv, db: &str, opts: &MoveOpts) -> Result<MoveProgress, String> {
    let rec = env.record(db)?;
    let report = check::run(ops, env, &rec, opts)?;
    let now = env.now();
    let tag = tag_for(now);
    let mut run = Run {
        ops,
        env,
        p: MoveProgress {
            id: format!("mv_{}_{}", rec.name, tag.to_ascii_lowercase()),
            db: rec.name.clone(),
            from: rec.major,
            to: report.to,
            tag,
            actor: opts.actor.clone(),
            state: MoveState::Checking,
            step: Step::Check,
            started_at: now,
            finished_at: None,
            last_error: None,
            from_port: report.from_port,
            to_port: report.to_port,
            target_created: false,
            quiesced: false,
            stopped_apps: Vec::new(),
            doors_paused: false,
            repointed: Vec::new(),
            roles_created: Vec::new(),
            dump_bytes: None,
            diff: Vec::new(),
            warnings: report.lines.iter().filter(|l| l.level == check::Level::Warn).map(|l| l.text.clone()).collect(),
            check: report.lines.clone(),
        },
    };
    run.env.progress(&run.p);
    // 1. check
    if report.refused() {
        let why = report
            .lines
            .iter()
            .filter(|l| l.level == check::Level::Refuse)
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join("; ");
        return Ok(run.finish(MoveState::Aborted, Some(format!("refused: {why}"))));
    }
    match steps(&mut run, &rec, &report, opts) {
        Ok(()) => Ok(run.p),
        Err(f) if f.is_done() => Ok(run.p),
        Err(StepFail(why)) => Ok(run.abort(why)),
    }
}

fn steps(run: &mut Run, rec: &DbRecord, report: &check::Report, opts: &MoveOpts) -> Result<(), StepFail> {
    let (db, from, to) = (rec.name.clone(), rec.major, report.to);
    let from_port = report.from_port.ok_or("the source cluster has no port on record")?;

    // 2. target: the source keeps serving.
    run.at(Step::Target, MoveState::Checking);
    let to_port = match report.to_port {
        Some(p) => p,
        None => {
            let rows = run.ops.clusters()?;
            let listening = run.ops.band_listening()?;
            free_band_port(&rows, &listening).ok_or("band_full: no free port in 5432–5449")?
        }
    };
    if !run.ops.installed()?.contains(&to) {
        run.ops.add_major(to).map_err(|e| format!("add-major {to}: {e}"))?;
    }
    run.ops.ensure_cluster(to, to_port).map_err(|e| format!("ensure-cluster {to}: {e}"))?;
    run.env.record_cluster(to, to_port);
    run.p.to_port = Some(to_port);
    // With the target installed, every extension must have a build there.
    let missing: Vec<String> = report
        .extensions
        .iter()
        .filter(|x| run.ops.extension_available(to, x) != Some(true))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(format!("extension_missing: no {to} build of {}", missing.join(", ")).into());
    }

    // 3. notice
    run.at(Step::Notice, MoveState::Checking);
    let pause = catalog::pause_estimate_secs(report.size_bytes);
    run.env.notice(
        &db,
        &format!("Database {db} moves to Postgres {to}: a pause of about {}–{} s starts in {} s.", pause.0, pause.1, if opts.now { 0 } else { NOTICE_SECS }),
    );
    run.env.pause_doors(&db, true);
    run.p.doors_paused = true;
    if !opts.now {
        run.env.sleep_secs(NOTICE_SECS);
    }

    // 4. roles, with their verifiers (refused before any change on a mismatch).
    run.at(Step::Roles, MoveState::Copying);
    let out = run.ops.copy_roles(from, to, &report.roles, false).map_err(|e| format!("copy roles: {e}"))?;
    run.p.roles_created = out["created"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let m = run.ops.sql(from, &db, &catalog::memberships_sql(&report.roles))?;
    let grants = catalog::membership_grants(&m, ok_name).ok_or("memberships: unexpected answer")?;
    if !grants.is_empty() {
        run.ops.sql(to, "postgres", &grants).map_err(|e| format!("memberships: {e}"))?;
    }

    // 5. quiesce
    run.at(Step::Quiesce, MoveState::Copying);
    let users = run.env.users(&db);
    run.p.stopped_apps = run.env.stop_apps(&users.apps).map_err(|e| format!("stop Apps: {e}"))?;
    run.ops.allow_conn(from, &db, ConnMode::Quiesce).map_err(|e| format!("quiesce: {e}"))?;
    run.p.quiesced = true;
    run.env.progress(&run.p);
    let counts_src = row_counts(run.ops, from, &db)?;
    let print_src = fingerprint(run.ops, from, &db)?;

    // 6. copy
    run.at(Step::Copy, MoveState::Copying);
    let props = run.ops.sql(from, &db, catalog::db_props_sql())?;
    let props: Value = serde_json::from_str(&props).map_err(|_| "database properties are not JSON")?;
    let acl = run.ops.sql(from, &db, catalog::datacl_sql())?;
    let settings = run.ops.sql(from, &db, catalog::db_settings_sql())?;
    let tag = run.p.tag.clone();
    run.p.dump_bytes = Some(run.ops.dump(from, to, &db, &tag).map_err(|e| format!("dump: {e}"))?);
    run.env.progress(&run.p);
    let create = catalog::create_db_sql(&db, &props, ok_name).ok_or("the database's owner or locale can't be carried")?;
    run.ops.sql(to, "postgres", &create).map_err(|e| format!("create the target database: {e}"))?;
    run.p.target_created = true;
    run.env.progress(&run.p);
    run.ops.restore(to, &db, &tag).map_err(|e| format!("restore: {e}"))?;
    let replay = catalog::datacl_replay(&db, &acl, ok_name).ok_or("database ACL: unexpected answer")?
        + &catalog::db_settings_replay(&db, &settings, ok_name).ok_or("database settings: unexpected answer")?;
    if !replay.is_empty() {
        run.ops.sql(to, "postgres", &replay).map_err(|e| format!("database grants: {e}"))?;
    }
    run.env.rerun_k2_setup(to, &db).map_err(|e| format!("K2 setup on {to}: {e}"))?;

    // 7. verify
    run.at(Step::Verify, MoveState::Verifying);
    let counts_dst = row_counts(run.ops, to, &db)?;
    let mut print_dst = fingerprint(run.ops, to, &db)?;
    let mut print_src = print_src;
    catalog::normalize_privileges(&mut print_src, from);
    catalog::normalize_privileges(&mut print_dst, from);
    let mut diff = catalog::diff(&json!({"rows": counts_src}), &json!({"rows": counts_dst}));
    diff.extend(catalog::diff(&print_src, &print_dst));
    for (role, pw) in run.env.logins(&db)? {
        if let Err(e) = run.ops.login(to, to_port, &db, &role, &pw) {
            diff.push(format!("login {role}: {e}"));
        }
    }
    if !diff.is_empty() {
        run.p.diff = diff.clone();
        return Err(format!("verify: {} difference(s), first: {}", diff.len(), diff[0]).into());
    }

    // 8. switch
    run.at(Step::Switch, MoveState::Switching);
    if let Err(e) = run.env.set_major(&db, to) {
        return Err(format!("switch the record: {e}").into());
    }
    match run.env.repoint(&db, from_port, to_port) {
        Ok(done) => run.p.repointed = done,
        Err((done, why)) => {
            // §13 row 8: reverse what was re-pointed, the record back.
            let back = run.env.unpoint(&db, &done, from_port, to_port).and_then(|_| run.env.set_major(&db, from));
            return match back {
                Ok(()) => {
                    let errs = run.undo_before_switch();
                    let msg = format!("re-point: {why}");
                    if errs.is_empty() {
                        run.env.audit("db-upgrade-rolled-back", json!({"db": db, "why": msg}));
                        run.finish(MoveState::RolledBack, Some(msg));
                        Err(StepFail::done())
                    } else {
                        Err(format!("{msg}; then: {}", errs.join("; ")).into())
                    }
                }
                Err(e) => {
                    run.p.repointed = done;
                    let msg = format!("re-point: {why}; reversing it failed: {e}");
                    run.env.audit("db-upgrade-failed", json!({"db": db, "why": msg}));
                    run.env.inbox(
                        &format!("Database {db} move failed"),
                        &format!(
                            "The move {from} → {to} stopped while re-pointing and the reversal failed: {e}. Steps re-pointed: {}. \
                             New moves are blocked until the owner fixes these and runs `k2 db upgrade --acknowledge-failed`.",
                            run.p.repointed.join(", ")
                        ),
                    );
                    run.finish(MoveState::Failed, Some(msg));
                    Err(StepFail::done())
                }
            };
        }
    }
    finish_forward(run, from, to, to_port)
}

impl StepFail {
    /// The machine already wrote the final state.
    fn done() -> Self {
        StepFail(String::from("\u{0}done"))
    }
    fn is_done(&self) -> bool {
        self.0 == "\u{0}done"
    }
}

/// Steps 9–10, also the recovery path after a restart at or after 8.
fn finish_forward(run: &mut Run, from: Major, to: Major, to_port: u16) -> Result<(), StepFail> {
    let db = run.p.db.clone();
    // 9. resume
    run.at(Step::Resume, MoveState::Switching);
    let failed = run.env.start_apps(&run.p.stopped_apps);
    if !failed.is_empty() {
        run.p.warnings.push(format!("Apps that didn't start: {}", failed.join(", ")));
    }
    run.p.stopped_apps.clear();
    run.env.pause_doors(&db, false);
    run.p.doors_paused = false;
    run.env.clear_notice(&db);
    if let Err(e) = run.ops.sql(to, &db, "ANALYZE;\n") {
        run.p.warnings.push(format!("ANALYZE on {to}: {e}"));
    }
    if let Err(e) = run.env.verify_k2(to, &db) {
        run.p.warnings.push(format!("K2 check through the guest door: {e}"));
    }
    // 10. record: the source stays, closed to everyone.
    run.at(Step::Record, MoveState::Switching);
    if let Err(e) = run.ops.allow_conn(from, &db, ConnMode::Off) {
        run.p.warnings.push(format!("close the kept source: {e}"));
    }
    run.p.quiesced = false;
    let now = run.env.now();
    let info = MovedInfo { from, at: now, until: now + ROLLBACK_WINDOW_SECS, tag: run.p.tag.clone(), source_state: SourceState::Kept };
    run.env.set_moved(&db, Some(&info)).map_err(|e| format!("record the move: {e}"))?;
    run.env.audit(
        "db-upgrade-move",
        json!({"db": db, "from": from, "to": to, "port": to_port, "actor": run.p.actor, "tag": run.p.tag, "warnings": run.p.warnings}),
    );
    run.env.inbox(
        &format!("Database {db} moved {from} → {to}"),
        &format!(
            "Database {db} moved {from} → {to} by {}. The {from} copy is kept (closed) until a rollback window ends in 14 days: \
             `k2 db upgrade {db} --rollback --yes` (writes made on {to} since the move are not carried back).",
            run.p.actor
        ),
    );
    run.p.step = Step::Done;
    run.finish(MoveState::Succeeded, None);
    Ok(())
}

/// After a daemon restart (§13 last row): a move found mid-way is undone
/// before the switch, finished at or after it.
pub fn recover(ops: &dyn ClusterOps, env: &dyn MoveEnv, p: MoveProgress) -> MoveProgress {
    let mut run = Run { ops, env, p };
    run.p.state = MoveState::Interrupted;
    env.progress(&run.p);
    if run.p.step < Step::Switch {
        return run.abort(format!("interrupted at {:?} by a daemon restart", run.p.step));
    }
    let (from, to) = (run.p.from, run.p.to);
    let (Some(from_port), Some(to_port)) = (run.p.from_port, run.p.to_port) else {
        return run.finish(MoveState::Failed, Some("interrupted after the switch with no ports on record".into()));
    };
    if run.p.step == Step::Switch {
        // Finish the re-point from the recorded list (idempotent steps).
        if let Err(e) = env.set_major(&run.p.db, to) {
            return run.finish(MoveState::Failed, Some(format!("recover: switch the record: {e}")));
        }
        match env.repoint(&run.p.db, from_port, to_port) {
            Ok(done) => run.p.repointed = done,
            Err((_, why)) => return run.finish(MoveState::Failed, Some(format!("recover: re-point: {why}"))),
        }
    }
    match finish_forward(&mut run, from, to, to_port) {
        Ok(()) => run.p,
        Err(f) if f.is_done() => run.p,
        Err(StepFail(why)) => run.finish(MoveState::Failed, Some(why)),
    }
}

// ── rollback and forget-old ─────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct RollbackOpts {
    pub actor: String,
    pub yes: bool,
    pub now: bool,
}

/// `--rollback <db> --yes` (§6.3): back to the kept source. Writes made on
/// the target since the move are not carried back.
pub fn rollback(ops: &dyn ClusterOps, env: &dyn MoveEnv, db: &str, o: &RollbackOpts) -> Result<Value, String> {
    if !o.yes {
        return Err("rollback_needs_yes: writes made since the move are lost on a rollback; re-run with --yes".into());
    }
    let rec = env.record(db)?;
    let from = rec.moved_from.ok_or("not_moved: this database has no move to roll back")?;
    if rec.source_state != SourceState::Kept {
        return Err("source_gone: the old copy was forgotten; nothing to roll back to".into());
    }
    let now = env.now();
    if rec.rollback_until.is_some_and(|u| now > u) {
        return Err("window_closed: the 14-day rollback window has ended".into());
    }
    let to = rec.major;
    let rows = ops.clusters()?;
    let from_port = port_of(&rows, from).ok_or("the old cluster is gone")?;
    let to_port = port_of(&rows, to).ok_or("the current cluster is gone")?;
    if !rows.iter().any(|r| r.major == from && r.online()) {
        ops.ensure_cluster(from, from_port).map_err(|e| format!("start the old cluster: {e}"))?;
    }
    env.notice(db, &format!("Database {db} rolls back to Postgres {from}; writes since the move are not carried back."));
    env.pause_doors(db, true);
    if !o.now {
        env.sleep_secs(NOTICE_SECS);
    }
    let users = env.users(db);
    let stopped = env.stop_apps(&users.apps).map_err(|e| {
        env.pause_doors(db, false);
        env.clear_notice(db);
        format!("stop Apps: {e}")
    })?;
    let undo = |why: String| -> String {
        let _ = ops.allow_conn(to, db, ConnMode::On);
        env.start_apps(&stopped);
        env.pause_doors(db, false);
        env.clear_notice(db);
        why
    };
    ops.allow_conn(to, db, ConnMode::Quiesce).map_err(|e| undo(format!("quiesce: {e}")))?;
    env.set_major(db, from).map_err(|e| undo(format!("record: {e}")))?;
    if let Err((done, why)) = env.repoint(db, to_port, from_port) {
        let back = env.unpoint(db, &done, to_port, from_port).and_then(|_| env.set_major(db, to));
        return Err(undo(match back {
            Ok(()) => format!("re-point: {why} (reversed; still on {to})"),
            Err(e) => format!("re-point: {why}; reversing it failed: {e}"),
        }));
    }
    ops.allow_conn(from, db, ConnMode::On).map_err(|e| format!("re-open the old copy: {e}"))?;
    // The target copy, renamed and closed, kept for inspection.
    let base: String = db.chars().take(63 - 17).collect();
    let kept = format!("{base}__k2rb_{now}");
    ops.sql(
        to,
        "postgres",
        &format!(
            "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity WHERE datname = {} AND pid <> pg_backend_pid();\n\
             ALTER DATABASE {} RENAME TO {};\n",
            catalog::quote_lit(db),
            catalog::quote_ident(db),
            catalog::quote_ident(&kept)
        ),
    )
    .map_err(|e| format!("rename the {to} copy: {e}"))?;
    let mut warnings = Vec::new();
    if let Err(e) = ops.allow_conn(to, &kept, ConnMode::Off) {
        warnings.push(format!("close the renamed copy: {e}"));
    }
    env.set_moved(db, None)?;
    let failed = env.start_apps(&stopped);
    if !failed.is_empty() {
        warnings.push(format!("Apps that didn't start: {}", failed.join(", ")));
    }
    env.pause_doors(db, false);
    env.clear_notice(db);
    env.audit("db-upgrade-rollback", json!({"db": db, "from": to, "to": from, "actor": o.actor, "keptAs": kept}));
    env.inbox(
        &format!("Database {db} rolled back {to} → {from}"),
        &format!("Database {db} is on Postgres {from} again (by {}). Writes made on {to} since the move were not carried back; the {to} copy is kept closed as {kept}.", o.actor),
    );
    Ok(json!({"db": db, "major": from, "port": from_port, "keptAs": kept, "warnings": warnings}))
}

#[derive(Debug, Clone, Default)]
pub struct ForgetOpts {
    pub actor: String,
    /// Owner only (the route checks).
    pub owner: bool,
    pub force_now: bool,
    pub move_busy: bool,
}

/// `--forget-old <db>` (§6.4, owner only): drop the kept source and its
/// dump. Never a package, never a cluster (Sew's own Postgres runs the
/// installed binaries; Sew 2026-10-20).
pub fn forget_old(ops: &dyn ClusterOps, env: &dyn MoveEnv, db: &str, o: &ForgetOpts) -> Result<Value, String> {
    if !o.owner {
        return Err("owner_only: deleting the old copy is the owner's call".into());
    }
    if o.move_busy {
        return Err("busy: a move is running".into());
    }
    let rec = env.record(db)?;
    let from = rec.moved_from.ok_or("not_moved: this database has no kept old copy")?;
    if rec.source_state != SourceState::Kept {
        return Err("nothing_kept: the old copy is already gone".into());
    }
    let now = env.now();
    if !o.force_now && rec.moved_at.is_some_and(|at| now - at < FORGET_MIN_AGE_SECS) {
        return Err("too_soon: wait 24 h after the move (or --force-now)".into());
    }
    // Healthy on the target first.
    ops.sql(rec.major, &rec.name, "SELECT 1;\n").map_err(|e| format!("not_healthy: the database doesn't answer on {}: {e}", rec.major))?;
    let dropped = ops.drop_db(from, db)?;
    if let Some(tag) = &rec.move_tag {
        ops.remove_dump(db, tag)?;
    }
    let info = MovedInfo {
        from,
        at: rec.moved_at.unwrap_or(now),
        until: rec.rollback_until.unwrap_or(now),
        tag: rec.move_tag.clone().unwrap_or_default(),
        source_state: SourceState::Forgotten,
    };
    env.set_moved(db, Some(&info))?;
    env.audit("db-upgrade-forget-old", json!({"db": db, "major": from, "actor": o.actor}));
    Ok(json!({"db": db, "dropped": dropped, "major": from, "packagesKept": true}))
}

/// `k2 db clusters tidy` (§4): K2 clusters with no K2 database and no kept
/// source go `manual` and stop. Never dropped; packages never removed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TidyPlan {
    pub stop: Vec<Major>,
    pub keep: Vec<(Major, String)>,
    pub packages_kept: bool,
    pub note: String,
}

pub fn tidy_plan(rows: &[ClusterRow], roles: &dyn Fn(Major) -> ClusterRole, dbs_on: &dyn Fn(Major) -> usize, kept_on: &dyn Fn(Major) -> usize, sew_major: Option<Major>) -> TidyPlan {
    let mut stop = Vec::new();
    let mut keep = Vec::new();
    for r in rows.iter().filter(|r| r.name == "main") {
        let why = if roles(r.major) != ClusterRole::K2 {
            Some("not K2's cluster".to_string())
        } else if dbs_on(r.major) > 0 {
            Some(format!("{} K2 database(s)", dbs_on(r.major)))
        } else if kept_on(r.major) > 0 {
            Some("a kept source for a rollback".to_string())
        } else {
            None
        };
        match why {
            Some(w) => keep.push((r.major, w)),
            None if r.online() => stop.push(r.major),
            None => {}
        }
    }
    let mut note = "Clusters are stopped and set manual, never dropped; K2 never removes Postgres packages.".to_string();
    if let Some(m) = sew_major {
        note.push_str(&format!(" Sew's own Postgres runs postgresql-{m}."));
    }
    TidyPlan { stop, keep, packages_kept: true, note }
}

/// Is this port one K2 may give a cluster?
pub fn port_ok(p: u16) -> bool {
    in_band(p)
}
