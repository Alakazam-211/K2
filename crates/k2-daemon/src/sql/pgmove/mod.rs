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
pub mod dual;
pub mod dual_k2;
pub mod helper_ops;
pub mod k2;
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
    /// Restore as the database owner (`role`/`password`): only DDL runs as
    /// the superuser (security review #1 H1, #2 F2).
    fn restore(&self, to: Major, db: &str, tag: &str, role: &str, password: &str) -> Result<(), String>;
    fn drop_db(&self, m: Major, db: &str) -> Result<bool, String>;
    fn remove_dump(&self, db: &str, tag: &str) -> Result<(), String>;
    /// Free bytes on the clusters' data filesystem.
    fn free_bytes(&self) -> Result<u64, String>;
    /// `None`: that major isn't installed (yet).
    fn extension_available(&self, m: Major, ext: &str) -> Option<bool>;
    /// A role logs in over TCP on `port` with its vaulted password.
    fn login(&self, m: Major, port: u16, db: &str, role: &str, password: &str) -> Result<(), String>;
    /// Bytes of move dumps kept on disk (the check's disk line, F7).
    fn moves_bytes(&self) -> u64 {
        0
    }
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
    /// Persist the move's row (`sql_moves`); called at every step. An error
    /// fails the step (review #2 F13).
    fn progress(&self, p: &MoveProgress) -> Result<(), String>;
    /// Fold the box's clusters into K2's records before a check (F9).
    fn refresh_clusters(&self) {}
    /// Other workspaces the database is shared with (`sql_grants`).
    fn shared_with(&self, _db: &str) -> Vec<String> {
        Vec::new()
    }
    /// Every App whose database is `db` (owner and grantee workspaces),
    /// with its live Seed's `requires.postgres` (prd-app-pg-choice-dualrun-v1
    /// PA3, PA6).
    fn app_ranges(&self, _db: &str) -> Vec<(String, Option<String>)> {
        Vec::new()
    }
    /// RAM outside the Apps' pool (a second cluster starts there, PA16);
    /// `None` when the box's memory can't be read.
    fn spare_ram_bytes(&self) -> Option<u64> {
        None
    }
    /// PA10: once the record points at the new major, each App comes back
    /// through a re-host (a fresh instance reads the new DSN, the health
    /// gate, no warm instance kept: PA11), tagged with `epoch` (the move
    /// id). Answers the Apps that didn't go live.
    fn rehost_apps(&self, apps: &[String], _epoch: &str) -> Vec<String> {
        self.start_apps(apps)
    }
    /// PA12: an App on this database being re-hosted right now (a move or
    /// cut-over waits for it).
    fn rehost_running(&self, _db: &str) -> Option<String> {
        None
    }
    /// The Apps (of these) that can't run two copies: a live cut-over stops
    /// them at the write block (PA10, PA31 step 2).
    fn one_copy_apps(&self, _apps: &[String]) -> Vec<String> {
        Vec::new()
    }
    /// A live dual-run of this database is open (PA27, PA35): moves and
    /// rollbacks refuse `dual_running`.
    fn dual_running(&self, _db: &str) -> bool {
        false
    }
    /// The server's owner hears it too (a live run's notes, PA29).
    fn owner_note(&self, _title: &str, _body: &str) {}
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
    /// `move` or `rollback`.
    #[serde(default = "kind_move")]
    pub kind: String,
    /// `copy` (0.46.3) or `dual` (a live run's cut-over,
    /// prd-app-pg-choice-dualrun-v1 PA31): the `sql_moves.method` column.
    #[serde(default = "method_copy")]
    pub method: String,
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
    /// The record points at `to`: no undo from here on (F4).
    #[serde(default)]
    pub switched: bool,
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
    /// A rollback: the newer copy's name once renamed and closed.
    #[serde(default)]
    pub kept_as: Option<String>,
    /// A rollback: the rolled-back move's dump (removed at the end).
    #[serde(default)]
    pub move_tag: Option<String>,
}

fn kind_move() -> String {
    "move".into()
}

fn method_copy() -> String {
    "copy".into()
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

/// Why a step failed. Before the switch the machine undoes; after it, it
/// never does (security review #2 F4): it finishes as `failed`.
struct StepFail(String);

impl<T: Into<String>> From<T> for StepFail {
    fn from(s: T) -> Self {
        StepFail(s.into())
    }
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

struct Run<'a> {
    ops: &'a dyn ClusterOps,
    env: &'a dyn MoveEnv,
    p: MoveProgress,
}

impl Run<'_> {
    /// Persist the step. A row K2 can't write is a failed step (review #2
    /// F13): recovery trusts these rows.
    fn at(&mut self, step: Step, state: MoveState) -> Result<(), StepFail> {
        self.p.step = step;
        self.p.state = state;
        self.save()
    }

    fn save(&self) -> Result<(), StepFail> {
        self.env.progress(&self.p).map_err(|e| StepFail(format!("progress row: {e}")))
    }

    fn finish(&mut self, state: MoveState, err: Option<String>) -> MoveProgress {
        self.p.state = state;
        self.p.last_error = err;
        self.p.finished_at = Some(self.env.now());
        if let Err(e) = self.env.progress(&self.p) {
            self.p.warnings.push(format!("progress row: {e}"));
        }
        self.p.clone()
    }

    /// Undo everything before the switch (§13 rows 1–7): drop the target
    /// copy, re-open the source, start the Apps, open the doors, clear the
    /// notice, remove the dump. Never once the record points at the target
    /// (review #2 F4): then the target is live and only the owner acts.
    fn undo_before_switch(&mut self) -> Vec<String> {
        let mut errs = Vec::new();
        let (db, from, to) = (self.p.db.clone(), self.p.from, self.p.to);
        let live_on_target = self.p.switched || self.env.record(&db).map(|r| r.major == to).unwrap_or(true);
        if self.p.target_created {
            if live_on_target {
                errs.push(format!("the record points at {to} (or can't be read): the {to} copy is live and was not dropped"));
            } else {
                // The helper drops only a closed or quiesced database.
                let dropped = self.ops.allow_conn(to, &db, ConnMode::Quiesce).and_then(|_| self.ops.drop_db(to, &db));
                match dropped {
                    Ok(_) => self.p.target_created = false,
                    Err(e) => errs.push(format!("drop the target copy: {e}")),
                }
            }
        }
        // 0.46.4 drill: the roles this move created on the target go too
        // (left behind, they blocked the next move of a database with the
        // same name). One that still holds something stays, warned.
        if !live_on_target && !self.p.roles_created.is_empty() {
            match catalog::drop_roles_sql(&self.p.roles_created, ok_name) {
                Some(sql) => match self.ops.sql(to, "postgres", &sql) {
                    Ok(_) => self.p.roles_created.clear(),
                    Err(e) => self.p.warnings.push(format!("drop the roles made on {to}: {e}")),
                },
                None => errs.push("roles made on the target: not role names".into()),
            }
        }
        if self.p.quiesced && !live_on_target {
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
        // An aborted move keeps no dump (review #2 F7).
        if self.p.dump_bytes.is_some() && !live_on_target {
            if let Err(e) = self.ops.remove_dump(&db, &self.p.tag) {
                self.p.warnings.push(format!("remove the dump: {e}"));
            }
        }
        errs
    }

    fn abort(&mut self, why: String) -> MoveProgress {
        let errs = self.undo_before_switch();
        if errs.is_empty() {
            self.env.audit(
                &format!("db-upgrade-{}-aborted", self.p.kind),
                json!({"db": self.p.db, "from": self.p.from, "to": self.p.to, "why": why}),
            );
            self.finish(MoveState::Aborted, Some(why))
        } else {
            let msg = format!("{why}; undo incomplete: {}", errs.join("; "));
            self.fail(msg)
        }
    }

    /// The latch: new moves wait for the owner (review #2 F6).
    fn fail(&mut self, msg: String) -> MoveProgress {
        self.env.audit(&format!("db-upgrade-{}-failed", self.p.kind), json!({"db": self.p.db, "why": msg}));
        self.env.inbox(
            &format!("Database {} {} failed", self.p.db, self.p.kind),
            &format!(
                "The {} {} → {} of {} stopped: {msg}. The record points at Postgres {}. Re-pointed: [{}]. \
                 New moves are blocked until the owner fixes this and runs `k2 db upgrade --acknowledge-failed`.",
                self.p.kind,
                self.p.from,
                self.p.to,
                self.p.db,
                self.env.record(&self.p.db).map(|r| r.major.to_string()).unwrap_or_else(|_| "?".into()),
                self.p.repointed.join(", ")
            ),
        );
        self.finish(MoveState::Failed, Some(msg))
    }

    /// After the switch: no undo, ever (review #2 F4). Best effort to bring
    /// the Apps and doors back on the target, then the latch.
    fn fail_after_switch(&mut self, why: String) -> MoveProgress {
        let db = self.p.db.clone();
        if !self.p.stopped_apps.is_empty() {
            let id = self.p.id.clone();
            let failed = self.env.rehost_apps(&self.p.stopped_apps, &id);
            if !failed.is_empty() {
                self.p.warnings.push(format!("Apps that didn't start: {}", failed.join(", ")));
            }
            self.p.stopped_apps.clear();
        }
        if self.p.doors_paused {
            self.env.pause_doors(&db, false);
            self.p.doors_paused = false;
        }
        self.env.clear_notice(&db);
        self.fail(format!("after the switch: {why}"))
    }

    fn settle(mut self, res: Result<(), StepFail>) -> MoveProgress {
        match res {
            Ok(()) => self.p,
            Err(f) if f.is_done() => self.p,
            Err(StepFail(why)) if self.p.switched => self.fail_after_switch(why),
            Err(StepFail(why)) => self.abort(why),
        }
    }
}

fn new_progress(kind: &str, db: &str, from: Major, to: Major, actor: &str, now: i64) -> MoveProgress {
    let tag = tag_for(now);
    MoveProgress {
        id: format!("{}_{}_{}", if kind == "rollback" { "rb" } else { "mv" }, db, tag.to_ascii_lowercase()),
        kind: kind.into(),
        method: method_copy(),
        db: db.into(),
        from,
        to,
        tag,
        actor: actor.into(),
        state: MoveState::Checking,
        step: Step::Check,
        started_at: now,
        finished_at: None,
        last_error: None,
        from_port: None,
        to_port: None,
        switched: false,
        target_created: false,
        quiesced: false,
        stopped_apps: Vec::new(),
        doors_paused: false,
        repointed: Vec::new(),
        roles_created: Vec::new(),
        dump_bytes: None,
        diff: Vec::new(),
        warnings: Vec::new(),
        check: Vec::new(),
        kept_as: None,
        move_tag: None,
    }
}

/// PA6: `--to` left out is the highest supported major every App on these
/// databases allows; K2's default when none does (the check then refuses,
/// naming the App).
pub fn default_target(dbs: &[String], env: &dyn MoveEnv) -> Major {
    let ranges: Vec<String> = dbs.iter().flat_map(|d| env.app_ranges(d)).filter_map(|(_, r)| r).collect();
    crate::apps::pg_choice::best_major(ranges.iter().map(String::as_str)).unwrap_or(k2_root_proto::pg_major::NEW_INSTALL)
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
    let mut p = new_progress("move", &rec.name, rec.major, report.to, &opts.actor, env.now());
    p.from_port = report.from_port;
    p.to_port = report.to_port;
    p.warnings = report.lines.iter().filter(|l| l.level == check::Level::Warn).map(|l| l.text.clone()).collect();
    p.check = report.lines.clone();
    let mut run = Run { ops, env, p };
    if let Err(StepFail(e)) = run.save() {
        return Err(format!("the move can't be recorded: {e}"));
    }
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
    let res = steps(&mut run, &rec, &report, opts);
    Ok(run.settle(res))
}

fn steps(run: &mut Run, rec: &DbRecord, report: &check::Report, opts: &MoveOpts) -> Result<(), StepFail> {
    let (db, from, to) = (rec.name.clone(), rec.major, report.to);
    let from_port = report.from_port.ok_or("the source cluster has no port on record")?;

    // 2. target: the source keeps serving.
    run.at(Step::Target, MoveState::Checking)?;
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
    run.p.doors_paused = true;
    run.at(Step::Notice, MoveState::Checking)?;
    let pause = catalog::pause_estimate_secs(report.size_bytes);
    run.env.notice(
        &db,
        &format!("Database {db} moves to Postgres {to}: a pause of about {}–{} s starts in {} s.", pause.0, pause.1, if opts.now { 0 } else { NOTICE_SECS }),
    );
    run.env.pause_doors(&db, true);
    if !opts.now {
        run.env.sleep_secs(NOTICE_SECS);
    }

    // 4. roles, with their verifiers (refused before any change on a mismatch).
    run.at(Step::Roles, MoveState::Copying)?;
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

    // 5. quiesce. The Apps about to stop are written down first (F13).
    let users = run.env.users(&db);
    run.p.stopped_apps = users.apps.clone();
    run.at(Step::Quiesce, MoveState::Copying)?;
    let stopped = run.env.stop_apps(&users.apps);
    match stopped {
        Ok(s) => run.p.stopped_apps = s,
        Err(e) => return Err(format!("stop Apps: {e}").into()),
    }
    run.p.quiesced = true;
    run.save()?;
    run.ops.allow_conn(from, &db, ConnMode::Quiesce).map_err(|e| format!("quiesce: {e}"))?;
    let counts_src = row_counts(run.ops, from, &db)?;
    let print_src = fingerprint(run.ops, from, &db)?;

    // 6. copy
    run.at(Step::Copy, MoveState::Copying)?;
    let props = run.ops.sql(from, &db, catalog::db_props_sql())?;
    let props: Value = serde_json::from_str(&props).map_err(|_| "database properties are not JSON")?;
    let acl = run.ops.sql(from, &db, catalog::datacl_sql())?;
    let settings = run.ops.sql(from, &db, catalog::db_settings_sql())?;
    // The data loads as the database owner, never the superuser (F2).
    let owner = props["owner"].as_str().ok_or("the database has no owner")?.to_string();
    let logins = run.env.logins(&db)?;
    let owner_pw = logins
        .iter()
        .find(|(r, _)| *r == owner)
        .map(|(_, p)| p.clone())
        .ok_or("the database's owner is not K2's migrator: K2 restores only as the owner")?;
    let tag = run.p.tag.clone();
    run.p.dump_bytes = Some(0);
    run.save()?;
    run.p.dump_bytes = Some(run.ops.dump(from, to, &db, &tag).map_err(|e| format!("dump: {e}"))?);
    let create = catalog::create_db_sql(&db, &props, ok_name).ok_or("the database's owner or locale can't be carried")?;
    run.p.target_created = true;
    run.save()?;
    run.ops.sql(to, "postgres", &create).map_err(|e| format!("create the target database: {e}"))?;
    run.ops.restore(to, &db, &tag, &owner, &owner_pw).map_err(|e| format!("restore: {e}"))?;
    let replay = catalog::datacl_replay(&db, &acl, ok_name).ok_or("database ACL: unexpected answer")?
        + &catalog::db_settings_replay(&db, &settings, ok_name).ok_or("database settings: unexpected answer")?;
    if !replay.is_empty() {
        run.ops.sql(to, "postgres", &replay).map_err(|e| format!("database grants: {e}"))?;
    }

    // 7. verify
    run.at(Step::Verify, MoveState::Verifying)?;
    let counts_dst = row_counts(run.ops, to, &db)?;
    let mut print_dst = fingerprint(run.ops, to, &db)?;
    let mut print_src = print_src;
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

    // 8. switch. The step is on record before the record moves (F4).
    run.at(Step::Switch, MoveState::Switching)?;
    if let Err(e) = run.env.set_major(&db, to) {
        return Err(format!("switch the record: {e}").into());
    }
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
            // §13 row 8: the record back first, then reverse what was
            // re-pointed from it.
            let back = run.env.set_major(&db, from).and_then(|_| run.env.unpoint(&db, &done, from_port, to_port));
            return match back {
                Ok(()) => {
                    run.p.switched = false;
                    run.p.target_created = true;
                    let errs = run.undo_before_switch();
                    let msg = format!("re-point: {why}");
                    if errs.is_empty() {
                        run.env.audit("db-upgrade-move-rolled-back", json!({"db": db, "why": msg}));
                        run.finish(MoveState::RolledBack, Some(msg));
                        Err(StepFail::done())
                    } else {
                        run.fail(format!("{msg}; then: {}", errs.join("; ")));
                        Err(StepFail::done())
                    }
                }
                Err(e) => {
                    run.p.repointed = done;
                    run.fail(format!("re-point: {why}; reversing it failed: {e}"));
                    Err(StepFail::done())
                }
            };
        }
    }
    finish_forward(run, from, to, to_port)
}

/// Steps 9–10, also the recovery path after a restart at or after 8. Any
/// error here finishes `failed`, never an undo (F4).
fn finish_forward(run: &mut Run, from: Major, to: Major, to_port: u16) -> Result<(), StepFail> {
    let db = run.p.db.clone();
    // 9. resume
    run.at(Step::Resume, MoveState::Switching)?;
    // PA10: back through a re-host on the new DSN (no warm instance, PA11).
    let failed = run.env.rehost_apps(&run.p.stopped_apps, &run.p.id);
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
    // K2's idempotent setup through the resolver, now pointing at `to`
    // (V34; what it sets rode the dump and was verified above).
    if let Err(e) = run.env.rerun_k2_setup(to, &db) {
        run.p.warnings.push(format!("K2 setup on {to}: {e}"));
    }
    if let Err(e) = run.env.verify_k2(to, &db) {
        run.p.warnings.push(format!("K2 check through the guest door: {e}"));
    }
    // 10. record: the source stays, closed to everyone.
    run.at(Step::Record, MoveState::Switching)?;
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

/// After a daemon restart (§13 last row): a move or rollback found
/// mid-way is undone when the record still points at its source, finished
/// forward when it points at its target (review #2 F4/F13). A record K2
/// can't read is held as `failed` for the owner.
pub fn recover(ops: &dyn ClusterOps, env: &dyn MoveEnv, p: MoveProgress) -> MoveProgress {
    let mut run = Run { ops, env, p };
    run.p.state = MoveState::Interrupted;
    if let Err(e) = env.progress(&run.p) {
        run.p.warnings.push(format!("progress row: {e}"));
    }
    let rec = match env.record(&run.p.db) {
        Ok(r) => r,
        Err(e) => return run.fail(format!("interrupted; the record can't be read: {e}")),
    };
    let switched = run.p.switched || rec.major == run.p.to;
    if run.p.kind == "rollback" {
        if !switched {
            let why = format!("interrupted at {:?} by a daemon restart", run.p.step);
            return rollback_undo(&mut run, why);
        }
        run.p.switched = true;
        let res = rollback_forward(&mut run);
        return run.settle(res);
    }
    if !switched {
        let why = format!("interrupted at {:?} by a daemon restart", run.p.step);
        return run.abort(why);
    }
    run.p.switched = true;
    run.p.target_created = false;
    let (from, to) = (run.p.from, run.p.to);
    let (Some(from_port), Some(to_port)) = (run.p.from_port, run.p.to_port) else {
        return run.fail("interrupted after the switch with no ports on record".into());
    };
    if run.p.step <= Step::Switch {
        // Finish the re-point (idempotent: it re-mints from the record).
        match env.repoint(&run.p.db, from_port, to_port) {
            Ok(done) => run.p.repointed = done,
            Err((_, why)) => return run.fail(format!("recover: re-point: {why}")),
        }
    }
    let res = finish_forward(&mut run, from, to, to_port);
    run.settle(res)
}

// ── rollback and forget-old ─────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct RollbackOpts {
    pub actor: String,
    pub yes: bool,
    pub now: bool,
    /// The owner (a shared database rolls back only by the owner: F11).
    pub owner: bool,
}

/// The record's move is consistent: it came from an older major and now
/// lives on a newer one (review #2 F5).
fn moved_cleanly(rec: &DbRecord) -> Result<Major, String> {
    let from = rec.moved_from.ok_or("not_moved: this database has no move")?;
    if from >= rec.major {
        return Err(format!(
            "inconsistent_record: {} says it moved from {from} but lives on {}; nothing done — the owner checks `k2 db upgrade {} --check` and the inbox",
            rec.name, rec.major, rec.name
        ));
    }
    Ok(from)
}

/// `--rollback <db> --yes` (§6.3): back to the kept source. Writes made on
/// the newer copy since the move are not carried back. A `sql_moves` row
/// of kind `rollback` with steps, boot recovery and the latch (F5).
pub fn rollback(ops: &dyn ClusterOps, env: &dyn MoveEnv, db: &str, o: &RollbackOpts) -> Result<MoveProgress, String> {
    if !o.yes {
        return Err("rollback_needs_yes: writes made since the move are lost on a rollback; re-run with --yes".into());
    }
    // Security review B8: a live run of this database owns it (its
    // freeze, slot and target copy): no rollback under it.
    if env.dual_running(db) {
        return Err(format!("{}: a live move of {db} is open; switch or abort it first (k2 db upgrade {db} --abort)", dual::DUAL_RUNNING));
    }
    let rec = env.record(db)?;
    let back_to = moved_cleanly(&rec)?;
    if rec.source_state != SourceState::Kept {
        return Err("source_gone: the old copy was forgotten; nothing to roll back to".into());
    }
    let now = env.now();
    if rec.rollback_until.is_some_and(|u| now > u) {
        return Err("window_closed: the 14-day rollback window has ended".into());
    }
    let shared = env.shared_with(db);
    if !shared.is_empty() && !o.owner {
        return Err(format!(
            "shared_database: {db} is shared with {} other workspace(s), whose writes since the move would be lost; the owner rolls it back",
            shared.len()
        ));
    }
    let cur = rec.major;
    let rows = ops.clusters()?;
    let back_port = port_of(&rows, back_to).ok_or("the old cluster is gone")?;
    let cur_port = port_of(&rows, cur).ok_or("the current cluster is gone")?;
    let mut p = new_progress("rollback", db, cur, back_to, &o.actor, now);
    p.from_port = Some(cur_port);
    p.to_port = Some(back_port);
    p.move_tag = rec.move_tag.clone();
    let mut run = Run { ops, env, p };
    run.save().map_err(|StepFail(e)| format!("the rollback can't be recorded: {e}"))?;
    let res = rollback_steps(&mut run, o.now);
    let p = match res {
        Ok(()) => run.p,
        Err(f) if f.is_done() => run.p,
        Err(StepFail(why)) if run.p.switched => run.fail_after_switch(why),
        Err(StepFail(why)) => rollback_undo(&mut run, why),
    };
    Ok(p)
}

fn rollback_steps(run: &mut Run, now: bool) -> Result<(), StepFail> {
    let (db, cur, back_to) = (run.p.db.clone(), run.p.from, run.p.to);
    let (cur_port, back_port) = (run.p.from_port.unwrap_or_default(), run.p.to_port.unwrap_or_default());
    run.at(Step::Target, MoveState::Checking)?;
    let rows = run.ops.clusters()?;
    if !rows.iter().any(|r| r.major == back_to && r.online()) {
        run.ops.ensure_cluster(back_to, back_port).map_err(|e| format!("start the old cluster: {e}"))?;
    }
    run.p.doors_paused = true;
    run.at(Step::Notice, MoveState::Checking)?;
    run.env.notice(&db, &format!("Database {db} rolls back to Postgres {back_to}; writes since the move are not carried back."));
    run.env.pause_doors(&db, true);
    if !now {
        run.env.sleep_secs(NOTICE_SECS);
    }
    let users = run.env.users(&db);
    run.p.stopped_apps = users.apps.clone();
    run.at(Step::Quiesce, MoveState::Copying)?;
    run.p.stopped_apps = run.env.stop_apps(&users.apps).map_err(|e| format!("stop Apps: {e}"))?;
    run.p.quiesced = true;
    run.save()?;
    run.ops.allow_conn(cur, &db, ConnMode::Quiesce).map_err(|e| format!("quiesce: {e}"))?;
    // Switch: on record first.
    run.at(Step::Switch, MoveState::Switching)?;
    run.env.set_major(&db, back_to).map_err(|e| format!("record: {e}"))?;
    run.p.switched = true;
    run.save()?;
    match run.env.repoint(&db, cur_port, back_port) {
        Ok(done) => run.p.repointed = done,
        Err((done, why)) => {
            let back = run.env.set_major(&db, cur).and_then(|_| run.env.unpoint(&db, &done, cur_port, back_port));
            return match back {
                Ok(()) => {
                    run.p.switched = false;
                    Err(format!("re-point: {why} (reversed; still on {cur})").into())
                }
                Err(e) => {
                    run.p.repointed = done;
                    run.fail(format!("re-point: {why}; reversing it failed: {e}"));
                    Err(StepFail::done())
                }
            };
        }
    }
    rollback_forward(run)
}

/// After the rollback's switch (and its recovery): re-open the old copy,
/// clear the moved state BEFORE the cosmetic rename (F5), resume, then
/// rename and close the newer copy (warnings only), remove the dump.
fn rollback_forward(run: &mut Run) -> Result<(), StepFail> {
    let (db, cur, back_to) = (run.p.db.clone(), run.p.from, run.p.to);
    run.at(Step::Resume, MoveState::Switching)?;
    run.ops.allow_conn(back_to, &db, ConnMode::On).map_err(|e| format!("re-open the old copy: {e}"))?;
    run.env.set_moved(&db, None).map_err(|e| format!("clear the move: {e}"))?;
    let failed = run.env.rehost_apps(&run.p.stopped_apps, &run.p.id);
    if !failed.is_empty() {
        run.p.warnings.push(format!("Apps that didn't start: {}", failed.join(", ")));
    }
    run.p.stopped_apps.clear();
    run.env.pause_doors(&db, false);
    run.p.doors_paused = false;
    run.env.clear_notice(&db);
    run.at(Step::Record, MoveState::Switching)?;
    let move_tag = run.p.move_tag.clone();
    let base: String = db.chars().take(63 - 17).collect();
    let kept = format!("{base}__k2rb_{}", run.env.now());
    match rename_closed(run.ops, run.env, cur, &db, &kept) {
        Ok(()) => run.p.kept_as = Some(kept.clone()),
        Err(e) => run.p.warnings.push(format!("rename the {cur} copy: {e} (it stays as {db}, quiesced, on {cur})")),
    }
    if let Some(tag) = move_tag {
        if let Err(e) = run.ops.remove_dump(&db, &tag) {
            run.p.warnings.push(format!("remove the move's dump: {e}"));
        }
    }
    run.env.audit(
        "db-upgrade-rollback",
        json!({"db": db, "from": cur, "to": back_to, "actor": run.p.actor, "keptAs": run.p.kept_as, "warnings": run.p.warnings}),
    );
    run.env.inbox(
        &format!("Database {db} rolled back {cur} → {back_to}"),
        &format!(
            "Database {db} is on Postgres {back_to} again (by {}). Writes made on {cur} since the move were not carried back; \
             the {cur} copy is kept closed as {}.",
            run.p.actor,
            run.p.kept_as.as_deref().unwrap_or("(not renamed; see the warnings)")
        ),
    );
    run.p.step = Step::Done;
    run.finish(MoveState::Succeeded, None);
    Ok(())
}

/// Undo a rollback before its switch: the newer copy serves again.
fn rollback_undo(run: &mut Run, why: String) -> MoveProgress {
    let db = run.p.db.clone();
    let mut errs = Vec::new();
    if run.p.quiesced {
        if let Err(e) = run.ops.allow_conn(run.p.from, &db, ConnMode::On) {
            errs.push(format!("re-open {}: {e}", run.p.from));
        }
        run.p.quiesced = false;
    }
    if !run.p.stopped_apps.is_empty() {
        let failed = run.env.start_apps(&run.p.stopped_apps);
        if !failed.is_empty() {
            errs.push(format!("Apps that didn't start again: {}", failed.join(", ")));
        }
        run.p.stopped_apps.clear();
    }
    if run.p.doors_paused {
        run.env.pause_doors(&db, false);
        run.p.doors_paused = false;
    }
    run.env.clear_notice(&db);
    if errs.is_empty() {
        run.env.audit("db-upgrade-rollback-aborted", json!({"db": db, "why": why}));
        run.finish(MoveState::Aborted, Some(why))
    } else {
        run.fail(format!("{why}; undo incomplete: {}", errs.join("; ")))
    }
}

/// Rename a database once its backends are gone (they leave
/// asynchronously after `pg_terminate_backend`; F5), then close it.
fn rename_closed(ops: &dyn ClusterOps, env: &dyn MoveEnv, m: Major, db: &str, to: &str) -> Result<(), String> {
    let lit = catalog::quote_lit(db);
    let mut last = String::new();
    for _ in 0..10 {
        let left = ops.sql(
            m,
            "postgres",
            &format!(
                "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity WHERE datname = {lit} AND pid <> pg_backend_pid();\n\
                 SELECT count(*) FROM pg_stat_activity WHERE datname = {lit} AND pid <> pg_backend_pid();\n"
            ),
        )?;
        if left.lines().last().map(str::trim) == Some("0") {
            match ops.sql(m, "postgres", &format!("ALTER DATABASE {} RENAME TO {};\n", catalog::quote_ident(db), catalog::quote_ident(to))) {
                Ok(_) => {
                    return ops.allow_conn(m, to, ConnMode::Off);
                }
                Err(e) => last = e,
            }
        }
        env.sleep_secs(1);
    }
    Err(if last.is_empty() { "sessions still open after 10 s".into() } else { last })
}

/// Every kept rollback copy of `db` (`<db>__k2rb_<ts>`) on the online
/// clusters.
pub fn rollback_copies(ops: &dyn ClusterOps, db: &str) -> Result<Vec<(Major, String)>, String> {
    let base: String = db.chars().take(63 - 17).collect();
    let mut out = Vec::new();
    for r in ops.clusters()?.iter().filter(|r| r.name == "main" && r.online()) {
        let t = ops.sql(
            r.major,
            "postgres",
            &format!(
                "SELECT coalesce(string_agg(datname, ','), '') FROM pg_database WHERE left(datname, {n}) = {b} AND substr(datname, {n1}, 7) = '__k2rb_';\n",
                n = base.len(),
                b = catalog::quote_lit(&base),
                n1 = base.len() + 1
            ),
        )?;
        out.extend(t.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| (r.major, s.to_string())));
    }
    Ok(out)
}

/// Drop rollback copies older than the window (F7): run at boot and after
/// moves. Answers what was dropped.
pub fn sweep_rollback_copies(ops: &dyn ClusterOps, now: i64) -> Vec<String> {
    let mut dropped = Vec::new();
    let Ok(rows) = ops.clusters() else { return dropped };
    for r in rows.iter().filter(|r| r.name == "main" && r.online()) {
        let Ok(t) = ops.sql(r.major, "postgres", "SELECT coalesce(string_agg(datname, ','), '') FROM pg_database WHERE datname LIKE '%\\_\\_k2rb\\_%';\n") else {
            continue;
        };
        for name in t.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let ts = name.rsplit("__k2rb_").next().and_then(|t| t.parse::<i64>().ok());
            if ts.is_some_and(|t| now - t > ROLLBACK_WINDOW_SECS) && ops.drop_db(r.major, name).is_ok() {
                dropped.push(format!("{}/{name}", r.major));
            }
        }
    }
    dropped
}

#[derive(Debug, Clone, Default)]
pub struct ForgetOpts {
    pub actor: String,
    /// Owner only (the route checks).
    pub owner: bool,
    pub force_now: bool,
}

/// `--forget-old <db>` (§6.4, owner only): drop the kept source, its dump
/// and any rollback copies. Refused unless the record is cleanly rolled
/// forward and the copy it would drop is the closed, idle old one (F5).
/// Never a package, never a cluster (Sew's own Postgres runs the
/// installed binaries; Sew 2026-10-20). The route holds the move lock
/// (F12).
pub fn forget_old(ops: &dyn ClusterOps, env: &dyn MoveEnv, db: &str, o: &ForgetOpts) -> Result<Value, String> {
    if !o.owner {
        return Err("owner_only: deleting the old copy is the owner's call".into());
    }
    let rec = env.record(db)?;
    let copies = rollback_copies(ops, db)?;
    let mut dropped_copies = Vec::new();
    let drop_copies = |out: &mut Vec<String>| -> Result<(), String> {
        for (m, name) in &copies {
            ops.drop_db(*m, name)?;
            out.push(format!("{m}/{name}"));
        }
        Ok(())
    };
    if rec.moved_from.is_none() || rec.source_state != SourceState::Kept {
        if copies.is_empty() {
            return Err("nothing_kept: no old copy and no rollback copy to delete".into());
        }
        drop_copies(&mut dropped_copies)?;
        env.audit("db-upgrade-forget-old", json!({"db": db, "rollbackCopies": dropped_copies, "actor": o.actor}));
        return Ok(json!({"db": db, "dropped": false, "rollbackCopies": dropped_copies, "packagesKept": true}));
    }
    let from = moved_cleanly(&rec)?;
    let now = env.now();
    if !o.force_now && rec.moved_at.is_some_and(|at| now - at < FORGET_MIN_AGE_SECS) {
        return Err("too_soon: wait 24 h after the move (or --force-now)".into());
    }
    // Healthy on the newer major first.
    ops.sql(rec.major, &rec.name, "SELECT 1;\n").map_err(|e| format!("not_healthy: the database doesn't answer on {}: {e}", rec.major))?;
    // The copy to drop must be the closed, idle old one.
    let state = ops.sql(
        from,
        "postgres",
        &format!(
            "SELECT (NOT datallowconn)::text || ',' || (SELECT count(*) FROM pg_stat_activity WHERE datname = {l}) FROM pg_database WHERE datname = {l};\n",
            l = catalog::quote_lit(db)
        ),
    )?;
    if state.trim() != "true,0" {
        return Err(format!(
            "source_live: the {from} copy of {db} accepts connections or has sessions ({}); nothing dropped",
            if state.trim().is_empty() { "missing" } else { state.trim() }
        ));
    }
    let dropped = ops.drop_db(from, db)?;
    if let Some(tag) = &rec.move_tag {
        ops.remove_dump(db, tag)?;
    }
    drop_copies(&mut dropped_copies)?;
    let info = MovedInfo {
        from,
        at: rec.moved_at.unwrap_or(now),
        until: rec.rollback_until.unwrap_or(now),
        tag: rec.move_tag.clone().unwrap_or_default(),
        source_state: SourceState::Forgotten,
    };
    env.set_moved(db, Some(&info))?;
    env.audit("db-upgrade-forget-old", json!({"db": db, "major": from, "rollbackCopies": dropped_copies, "actor": o.actor}));
    Ok(json!({"db": db, "dropped": dropped, "major": from, "rollbackCopies": dropped_copies, "packagesKept": true}))
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
