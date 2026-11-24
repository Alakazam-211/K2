//! A Postgres server K2 runs that is older than its installed package.
//!
//! unattended-upgrades can move a Postgres package (17.11 to 17.12) under a
//! running server. Debian restarts the `postgresql@` clusters it manages;
//! a server K2 started under its own unit (Sew's `sew-db`) keeps running the
//! old binary, and a new backend then can't load the new package's
//! libraries (plpgsql.so: "undefined symbol"), so a migration and its
//! rollback fail.
//!
//! For each server K2 runs (Sew's `sew-db`, the `k2 db` clusters, named
//! ones like the feedback cluster) this compares the running postmaster's
//! start (its unit's `ExecMainStartTimestamp`) with the package's install
//! time: `dpkg-query`'s `db-fsys:Last-Modified`, else the mtime of
//! `/var/lib/dpkg/info/postgresql-<M>.list`, else the binary's mtime. The
//! major comes from the postmaster's command line
//! (`/usr/lib/postgresql/<M>/bin/postgres`), else the unit name.
//!
//! It never restarts anything. The health loop ([`spawn`]) checks on boot
//! and every [`EVERY_SECS`], keeps the last answer for boot-status,
//! `k2 db status` and `k2 hostchat status`, and files one owner notice per
//! stale server start.

use std::sync::Mutex;

use serde_json::{json, Value};
use k2_core::brand::CLI_NAME;
use k2_core::cli_name;

/// The health loop's cadence (a few forks per server: cheap).
pub const EVERY_SECS: u64 = 15 * 60;

/// What K2 reads from the box. Real: [`Real`]; tests fake every answer.
pub trait StaleOps: Sync {
    /// The unit's main PID and its start (unix seconds), or None when the
    /// unit isn't running.
    fn unit_start(&self, unit: &str) -> Option<(u32, i64)>;
    /// `/proc/<pid>/cmdline`, NULs as spaces.
    fn cmdline(&self, pid: u32) -> Option<String>;
    /// `dpkg-query -W -f '${db-fsys:Last-Modified}' <pkg>` (unix seconds).
    fn dpkg_last_modified(&self, pkg: &str) -> Option<i64>;
    /// A file's mtime (unix seconds).
    fn mtime(&self, path: &str) -> Option<i64>;
    /// `<postgres binary> -V`'s version (`17.12`).
    fn binary_version(&self, path: &str) -> Option<String>;
}

/// One server's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// `sew-db`, `17/main`, `17/feedback`.
    pub name: String,
    pub unit: String,
    pub major: Option<u32>,
    pub started_at: i64,
    pub package: Option<String>,
    pub installed_at: Option<i64>,
    /// `dpkg`, `dpkg-list` or `binary`.
    pub installed_from: Option<&'static str>,
    /// The installed binary's version (`postgres -V`).
    pub installed_version: Option<String>,
    /// The package changed after the server started. None: K2 couldn't tell.
    pub stale: Option<bool>,
    /// The person's fix, when stale.
    pub fix: String,
}

impl Finding {
    pub fn is_stale(&self) -> bool {
        self.stale == Some(true)
    }

    pub fn json(&self) -> Value {
        json!({
            "name": self.name,
            "unit": self.unit,
            "major": self.major,
            "startedAt": self.started_at,
            "package": self.package,
            "installedAt": self.installed_at,
            "installedFrom": self.installed_from,
            "installedVersion": self.installed_version,
            "stale": self.stale,
            "fix": if self.is_stale() { Value::from(self.fix.clone()) } else { Value::Null },
        })
    }

    /// One line for a person.
    pub fn line(&self) -> String {
        let v = self.installed_version.as_deref().map(|v| format!(" ({v})")).unwrap_or_default();
        format!(
            "{} runs an older Postgres than the installed {}{v}: it started before the package was upgraded, so it \
             can fail to load the new libraries. {}",
            self.name,
            self.package.as_deref().unwrap_or("package"),
            self.fix
        )
    }

    /// The notice key: one notice per server start per package install.
    pub fn key(&self) -> String {
        format!("{}@{}>{}", self.unit, self.started_at, self.installed_at.unwrap_or(0))
    }
}

/// The Postgres major in a postmaster command line or a unit name.
pub fn major_of(cmdline: Option<&str>, unit: &str) -> Option<u32> {
    let from_cmd = cmdline.and_then(|c| {
        c.split_whitespace().find_map(|w| {
            let rest = w.strip_prefix("/usr/lib/postgresql/")?;
            let (m, tail) = rest.split_once('/')?;
            (tail == "bin/postgres").then(|| m.parse::<u32>().ok()).flatten()
        })
    });
    from_cmd.or_else(|| {
        let rest = unit.strip_prefix("postgresql@")?;
        rest.split(['-', '.']).next()?.parse::<u32>().ok()
    })
}

/// Check one running server. None: its unit isn't running.
pub fn check(ops: &dyn StaleOps, name: &str, unit: &str, fix: &str) -> Option<Finding> {
    let (pid, started_at) = ops.unit_start(unit)?;
    let major = major_of(ops.cmdline(pid).as_deref(), unit);
    let mut f = Finding {
        name: name.into(),
        unit: unit.into(),
        major,
        started_at,
        package: None,
        installed_at: None,
        installed_from: None,
        installed_version: None,
        stale: None,
        fix: fix.into(),
    };
    let Some(m) = major else { return Some(f) };
    let pkg = format!("postgresql-{m}");
    let binary = format!("/usr/lib/postgresql/{m}/bin/postgres");
    let (at, from) = match ops.dpkg_last_modified(&pkg) {
        Some(t) => (Some(t), Some("dpkg")),
        None => match ops.mtime(&format!("/var/lib/dpkg/info/{pkg}.list")) {
            Some(t) => (Some(t), Some("dpkg-list")),
            None => match ops.mtime(&binary) {
                Some(t) => (Some(t), Some("binary")),
                None => (None, None),
            },
        },
    };
    f.package = Some(pkg);
    f.installed_at = at;
    f.installed_from = from;
    f.installed_version = ops.binary_version(&binary);
    f.stale = at.map(|t| t > started_at);
    Some(f)
}

// ── the servers K2 runs ──────────────────────────────────────────────────

/// The person's fix for a stale sew-db (the root helper has no verb that
/// restarts sew-db alone, so a person with root runs it).
pub const SEW_DB_FIX: &str = concat!("Restart it, then Sew: sudo systemctl restart sew-db.service, then ", cli_name!(), " hostchat restart.");

/// Sew's `sew-db`, when K2 has a Sew install record.
pub fn check_sew_db(ops: &dyn StaleOps) -> Option<Finding> {
    check(ops, "sew-db", k2_root_proto::sew::UNIT_DB, SEW_DB_FIX)
}

/// The `k2 db` clusters K2 recorded (named ones included).
pub fn check_sql_clusters(ops: &dyn StaleOps) -> Vec<Finding> {
    crate::sql::cluster::list()
        .into_iter()
        .filter(crate::sql::cluster::Cluster::holds_k2_databases)
        .filter_map(|c| {
            let label = c.label();
            let fix = if c.name == k2_root_proto::pg_major::CLUSTER_NAME {
                format!("Restart it: sudo systemctl restart {}.", c.unit)
            } else {
                format!("Restart it: {CLI_NAME} db cluster restart {label}.")
            };
            check(ops, &label, &c.unit, &fix)
        })
        .collect()
}

/// Every server K2 runs on this box.
pub fn check_all(ops: &dyn StaleOps) -> Vec<Finding> {
    let mut v = Vec::new();
    if crate::sew::store::load().is_some_and(|r| r.status != "removed") {
        v.extend(check_sew_db(ops));
    }
    v.extend(check_sql_clusters(ops));
    v
}

// ── the last answer + one notice per stale start ─────────────────────────

#[derive(Debug, Clone)]
struct Report {
    checked_at: i64,
    findings: Vec<Finding>,
}

static LAST: Mutex<Option<Report>> = Mutex::new(None);

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Keep `findings` as the last answer.
pub fn remember(findings: Vec<Finding>) {
    *LAST.lock().unwrap_or_else(|p| p.into_inner()) = Some(Report { checked_at: now(), findings });
}

/// The stale servers from the last check. Never checks.
pub fn last_stale() -> Vec<Finding> {
    LAST.lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|r| r.findings.iter().filter(|f| f.is_stale()).cloned().collect())
        .unwrap_or_default()
}

/// boot-status `pgStale`: null until the first check (and off Linux), else
/// `{checkedAt, stale: [{name, unit, fix, …}]}`.
pub fn boot_status_value() -> Value {
    match LAST.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
        None => Value::Null,
        Some(r) => json!({
            "checkedAt": r.checked_at,
            "stale": r.findings.iter().filter(|f| f.is_stale()).map(Finding::json).collect::<Vec<_>>(),
        }),
    }
}

/// Where notices go and which ones went (tests fake both).
pub trait Notices {
    fn sent(&self) -> Vec<String>;
    fn save_sent(&self, keys: &[String]);
    fn notify(&self, title: &str, body: &str);
}

/// At most this many keys are kept.
const SENT_KEEP: usize = 32;

/// One notice for each stale server start not noticed before. Returns the
/// keys noticed now.
pub fn notice_new(findings: &[Finding], n: &dyn Notices) -> Vec<String> {
    let mut sent = n.sent();
    let mut now_sent = Vec::new();
    for f in findings.iter().filter(|f| f.is_stale()) {
        let k = f.key();
        if sent.contains(&k) {
            continue;
        }
        n.notify(&format!("{} runs an outdated Postgres", f.name), &f.line());
        sent.push(k.clone());
        now_sent.push(k);
    }
    if !now_sent.is_empty() {
        let keep = sent.len().saturating_sub(SENT_KEEP);
        n.save_sent(&sent[keep..]);
    }
    now_sent
}

// ── production ───────────────────────────────────────────────────────────

/// The real box.
pub struct Real;

pub static REAL: Real = Real;

fn out(cmd: &str, args: &[&str]) -> Option<String> {
    let o = k2_core::bg_proc::command(cmd).args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// `systemctl show`'s `MainPID=` and `ExecMainStartTimestamp=@<secs>`.
pub fn parse_unit_start(show: &str) -> Option<(u32, i64)> {
    let mut pid = None;
    let mut at = None;
    for l in show.lines() {
        if let Some(v) = l.strip_prefix("MainPID=") {
            pid = v.trim().parse::<u32>().ok().filter(|p| *p > 0);
        } else if let Some(v) = l.strip_prefix("ExecMainStartTimestamp=") {
            at = v.trim().strip_prefix('@').and_then(|s| s.parse::<i64>().ok());
        }
    }
    Some((pid?, at?))
}

/// `postgres (PostgreSQL) 17.12 (Ubuntu 17.12-1.pgdg24.04+1)` → `17.12`.
pub fn parse_postgres_version(v: &str) -> Option<String> {
    let rest = v.split("(PostgreSQL)").nth(1)?;
    rest.split_whitespace().next().map(str::to_string)
}

impl StaleOps for Real {
    fn unit_start(&self, unit: &str) -> Option<(u32, i64)> {
        let show = out("systemctl", &["show", unit, "-p", "MainPID", "-p", "ExecMainStartTimestamp", "--timestamp=unix"])?;
        parse_unit_start(&show)
    }
    fn cmdline(&self, pid: u32) -> Option<String> {
        let b = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(String::from_utf8_lossy(&b).replace('\0', " ").trim().to_string())
    }
    fn dpkg_last_modified(&self, pkg: &str) -> Option<i64> {
        out("dpkg-query", &["-W", "-f", "${db-fsys:Last-Modified}", pkg])?.trim().parse::<i64>().ok()
    }
    fn mtime(&self, path: &str) -> Option<i64> {
        let m = std::fs::metadata(path).ok()?.modified().ok()?;
        m.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
    }
    fn binary_version(&self, path: &str) -> Option<String> {
        parse_postgres_version(&out(path, &["-V"])?)
    }
}

/// Owner notices from "K2 Postgres", the keys in `~/.k2/pg-stale-notices.json`.
struct RealNotices;

fn sent_path() -> std::path::PathBuf {
    k2_core::paths::k2_home().join("pg-stale-notices.json")
}

impl Notices for RealNotices {
    fn sent(&self) -> Vec<String> {
        std::fs::read_to_string(sent_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    fn save_sent(&self, keys: &[String]) {
        let body = serde_json::to_vec(keys).unwrap_or_default();
        if let Err(e) = k2_core::fs_atomic::atomic_write_mode(&sent_path(), &body, 0o600) {
            k2_core::log_debug!("[pg-stale] save notices: {e}");
        }
    }
    fn notify(&self, title: &str, body: &str) {
        #[cfg(not(test))]
        if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], "K2 Postgres", title, body) {
            k2_core::log_debug!("[pg-stale] owner notice '{title}': {e}");
        }
        #[cfg(test)]
        let _ = (title, body);
    }
}

/// One pass: check, remember, notice.
pub fn tick() -> Vec<Finding> {
    let f = check_all(&REAL);
    for s in f.iter().filter(|f| f.is_stale()) {
        k2_core::log_debug!("[pg-stale] {}", s.line());
    }
    notice_new(&f, &RealNotices);
    remember(f.clone());
    f
}

/// The health loop: on boot, then every [`EVERY_SECS`]. Linux only.
pub fn spawn() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let r = std::thread::Builder::new().name("pg-stale".into()).spawn(|| loop {
        if k2_core::db::try_shared().is_some() {
            tick();
        }
        std::thread::sleep(std::time::Duration::from_secs(EVERY_SECS));
    });
    if let Err(e) = r {
        k2_core::log_debug!("[pg-stale] spawn: {e}");
    }
}

/// A fake box for tests (here and Sew's `FakeOps`).
#[cfg(test)]
pub mod fake {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::StaleOps;

    /// A fake box: units with a start, command lines, package times.
    #[derive(Default)]
    pub struct Fake {
        pub units: Mutex<HashMap<String, (u32, i64)>>,
        pub cmdlines: Mutex<HashMap<u32, String>>,
        pub dpkg: Mutex<HashMap<String, i64>>,
        pub mtimes: Mutex<HashMap<String, i64>>,
        pub versions: Mutex<HashMap<String, String>>,
    }

    impl StaleOps for Fake {
        fn unit_start(&self, unit: &str) -> Option<(u32, i64)> {
            self.units.lock().unwrap().get(unit).copied()
        }
        fn cmdline(&self, pid: u32) -> Option<String> {
            self.cmdlines.lock().unwrap().get(&pid).cloned()
        }
        fn dpkg_last_modified(&self, pkg: &str) -> Option<i64> {
            self.dpkg.lock().unwrap().get(pkg).copied()
        }
        fn mtime(&self, path: &str) -> Option<i64> {
            self.mtimes.lock().unwrap().get(path).copied()
        }
        fn binary_version(&self, path: &str) -> Option<String> {
            self.versions.lock().unwrap().get(path).cloned()
        }
    }
}

#[cfg(test)]
#[path = "pg_stale_tests.rs"]
mod tests;
