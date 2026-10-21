//! Box usage, the measuring half (prd-app-resources-disk-guard-v1 §5,
//! Slice A1, lane M). The view (`resources.rs`) only reads what this
//! publishes ([`k2_core::disk_guard::publish_snapshot`]); it never scans.
//!
//! What is measured, and how often (§5.1):
//!
//! | Number | Source | Every |
//! |---|---|---|
//! | App disk, process engine | a low-priority walk of `~/.k2/apps/<id>` | 30 min |
//! | App disk, walled Apps, images, engine Seeds | the engine's `disk-usage` verb, **only when its `version` lists it** (no protocol bump) | 30 min |
//! | `~/.k2` parts | a walk of `~/.k2` (Seeds excluded: their sizes are in SQLite) | 30 min |
//! | Postgres | one root-helper `psql -d postgres -tA`: `pg_database_size` per database, the WAL, `data_directory` | 60 min |
//! | Workspace folders | a walk of each `projects.path` (60 s per folder, 10 min per sweep; `partial` past it) | 6 h, and `--rescan` |
//! | Agent tools | walks of `~/.claude`, `~/.codex`, `~/.cache`, `~/.npm`, `~/.bun`, `~/.grok` | 6 h |
//! | Mail store | the hosted-mail backup observer's latest `storeBytes` (never walked again) | daily (already runs) |
//! | App and `--cmd` memory, states, owners | the supervisor's live instances (`stats`), `/proc` by session | every 60 s (the watch) and on request (10 s) |
//!
//! The walk ([`walk`]) is Rust in the scanner's own thread, which runs at
//! nice 19 and idle I/O on Linux (background QoS on macOS): `lstat` only
//! (symlinks never followed), allocated blocks like `du`, a hard-linked
//! file once, one filesystem (`du -x`), and a nested workspace's folder is
//! skipped by the outer one (each byte counted once).
//!
//! Results live in memory and in `~/.k2/resources-cache.json`, so a
//! restarted daemon shows the last numbers at once. Nothing here needs a
//! table, and nothing here writes outside that cache file.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::deploy::store;
use k2_core::disk_guard::{self as dg, Consumer, Owner, ScanTimes, Snapshot, WillClear};
use k2_root_proto::engine::{self as wire, DiskUsage, Request};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value as J;

/// `~/.k2/resources-cache.json`.
pub const CACHE_FILE: &str = "resources-cache.json";

/// Cadences (§5.1).
pub const APPS_EVERY_SECS: i64 = 30 * 60;
pub const DATABASES_EVERY_SECS: i64 = 60 * 60;
pub const FOLDERS_EVERY_SECS: i64 = 6 * 3600;
/// Time limits (§5.2): per folder, and per sweep of folders.
pub const FOLDER_LIMIT: Duration = Duration::from_secs(60);
pub const SWEEP_LIMIT: Duration = Duration::from_secs(10 * 60);
/// The first scans after boot wait this long (boot stays quick).
const BOOT_DELAY_SECS: i64 = 120;
/// The agent tools' folders under `$HOME` (§5.1).
pub const TOOL_DIRS: &[&str] = &[".claude", ".codex", ".cache", ".npm", ".bun", ".grok"];
/// How many top-level parts a workspace row keeps (the view shows three).
const WORKSPACE_PARTS: usize = 3;
/// The walled engine's tree (k2app's home).
pub const K2APP_HOME: &str = "/var/lib/k2app";
/// Where Postgres keeps its data when the helper hasn't said.
pub const PG_DEFAULT_DIR: &str = "/var/lib/postgresql";

// ── the cache ────────────────────────────────────────────────────────

/// One walked folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FolderScan {
    pub bytes: u64,
    /// Per top-level child.
    pub parts: BTreeMap<String, u64>,
    pub partial: bool,
    pub scanned_at: i64,
}

/// The walled engine's last answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EngineScan {
    /// `ok` | `missing` (no walled engine) | `old` (no `disk-usage` verb) |
    /// `error` (it answered, but not this).
    pub status: String,
    pub at: i64,
    pub usage: Option<DiskUsage>,
    pub error: Option<String>,
    /// The engine lists `seeds-gc` (the sweep may call it).
    pub seeds_gc: bool,
}

/// Postgres sizes (one helper call).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DbScan {
    pub databases: BTreeMap<String, u64>,
    pub wal_bytes: u64,
    pub data_directory: Option<String>,
    pub at: i64,
    pub error: Option<String>,
}

/// Everything the scanner knows (`~/.k2/resources-cache.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Cache {
    /// Process-engine Apps (walked), by App id.
    pub apps: BTreeMap<String, FolderScan>,
    pub apps_at: Option<i64>,
    pub engine: Option<EngineScan>,
    pub k2: Option<FolderScan>,
    pub workspaces: BTreeMap<String, FolderScan>,
    pub workspaces_at: Option<i64>,
    pub tools: BTreeMap<String, FolderScan>,
    pub databases: Option<DbScan>,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Cache::default()))
}

/// A copy of what the scanner knows now.
pub fn cache_now() -> Cache {
    cache().lock().unwrap_or_else(|p| p.into_inner()).clone()
}

fn update_cache(f: impl FnOnce(&mut Cache)) {
    let mut c = cache().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut c);
}

pub fn cache_path() -> PathBuf {
    k2_core::paths::k2_home().join(CACHE_FILE)
}

/// Read the cache file (missing or unreadable = empty; logged).
pub fn load_cache_file(path: &Path) -> Cache {
    match std::fs::read_to_string(path) {
        Err(_) => Cache::default(),
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            k2_core::log_debug!("[disk-scan] {} unreadable ({e}); starting empty", path.display());
            Cache::default()
        }),
    }
}

/// Write the cache file atomically (temp + rename).
pub fn save_cache_file(path: &Path, c: &Cache) -> Result<(), String> {
    let dir = path.parent().ok_or("cache path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{CACHE_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(c).map_err(|e| e.to_string())?).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))
}

fn persist() {
    let c = cache_now();
    if let Err(e) = save_cache_file(&cache_path(), &c) {
        k2_core::log_debug!("[disk-scan] cache: {e}");
    }
}

// ── the walk ─────────────────────────────────────────────────────────

/// How a walk is bounded.
pub struct WalkOpts<'a> {
    /// Folders inside the root that belong to someone else (a nested
    /// workspace, the Seed store): not entered, not counted.
    pub skip: &'a [PathBuf],
    /// Stop here and mark the result `partial`.
    pub deadline: Instant,
}

/// Walk `root` like `du -x` (allocated bytes, `lstat` only, a hard link
/// once, one filesystem), with totals per top-level child. `None` when the
/// root isn't there.
pub fn walk(root: &Path, opts: &WalkOpts<'_>, now: i64) -> Option<FolderScan> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::symlink_metadata(root).ok()?;
        let mut out = FolderScan { bytes: meta.blocks().saturating_mul(512), scanned_at: now, ..FolderScan::default() };
        if !meta.is_dir() {
            return Some(out);
        }
        let dev = meta.dev();
        let skip: BTreeSet<&Path> = opts.skip.iter().map(PathBuf::as_path).collect();
        let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
        // (folder, the top-level part it counts toward).
        let mut stack: Vec<(PathBuf, Option<String>)> = vec![(root.to_path_buf(), None)];
        while let Some((dir, part)) = stack.pop() {
            if Instant::now() >= opts.deadline {
                out.partial = true;
                break;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if skip.contains(p.as_path()) {
                    continue;
                }
                let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
                if m.dev() != dev {
                    continue;
                }
                if !m.is_dir() && m.nlink() > 1 && !seen.insert((m.dev(), m.ino())) {
                    continue;
                }
                let b = m.blocks().saturating_mul(512);
                let part = part.clone().unwrap_or_else(|| e.file_name().to_string_lossy().to_string());
                out.bytes = out.bytes.saturating_add(b);
                *out.parts.entry(part.clone()).or_insert(0) += b;
                if m.is_dir() {
                    stack.push((p, Some(part)));
                }
            }
        }
        Some(out)
    }
    #[cfg(not(unix))]
    {
        let _ = (root, opts, now);
        None
    }
}

/// Keep the `n` biggest parts.
pub fn top_parts(parts: &BTreeMap<String, u64>, n: usize) -> BTreeMap<String, u64> {
    let mut v: Vec<(&String, &u64)> = parts.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    v.into_iter().take(n).map(|(k, v)| (k.clone(), *v)).collect()
}

/// `~/.k2`'s parts in the view's words: `apps`, `db`, `publishLogs`,
/// `other` (Seeds are added from SQLite).
pub fn k2_parts(raw: &BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for (name, b) in raw {
        let key = if name == "apps" {
            "apps"
        } else if name == "publish-logs" {
            "publishLogs"
        } else if [".db", ".db-wal", ".db-shm", ".redb", ".sqlite"].iter().any(|s| name.ends_with(s)) {
            "db"
        } else {
            "other"
        };
        *out.entry(key.into()).or_insert(0) += b;
    }
    out
}

/// The scanner thread's priority: nice 19 and idle I/O on Linux (this
/// thread only), background QoS on macOS.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        if libc::setpriority(libc::PRIO_PROCESS, tid, 19) != 0 {
            k2_core::log_debug!("[disk-scan] nice 19: {}", std::io::Error::last_os_error());
        }
        // IOPRIO_WHO_PROCESS = 1, IOPRIO_CLASS_IDLE = 3 << 13.
        if libc::syscall(libc::SYS_ioprio_set, 1, tid as libc::c_long, (3 << 13) as libc::c_long) != 0 {
            k2_core::log_debug!("[disk-scan] idle I/O: {}", std::io::Error::last_os_error());
        }
    }
    #[cfg(target_os = "macos")]
    unsafe {
        if libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG) != 0 {
            k2_core::log_debug!("[disk-scan] background QoS: {}", std::io::Error::last_os_error());
        }
    }
}

// ── the walled engine ────────────────────────────────────────────────

/// Ask the engine at `socket` for `disk-usage`, only when its `version`
/// lists the verb.
#[cfg(unix)]
pub fn engine_scan_at(socket: &Path, now: i64) -> EngineScan {
    let v = match super::guarded::call_at(socket, &Request::Version, None, Duration::from_secs(10)) {
        Ok(v) => v,
        Err(e) => return EngineScan { status: "missing".into(), at: now, error: Some(e.message), ..EngineScan::default() },
    };
    let verbs: Vec<&str> = v["verbs"].as_array().map(|a| a.iter().filter_map(J::as_str).collect()).unwrap_or_default();
    let seeds_gc = verbs.contains(&wire::VERB_SEEDS_GC);
    if !verbs.contains(&wire::VERB_DISK_USAGE) {
        return EngineScan { status: "old".into(), at: now, seeds_gc, ..EngineScan::default() };
    }
    let timeout = Duration::from_secs(wire::DISK_USAGE_SECS + 30);
    match super::guarded::call_at(socket, &Request::DiskUsage, None, timeout) {
        Ok(a) => match serde_json::from_value::<DiskUsage>(a) {
            Ok(u) => EngineScan { status: "ok".into(), at: now, usage: Some(u), error: None, seeds_gc },
            Err(e) => EngineScan { status: "error".into(), at: now, error: Some(format!("unreadable disk-usage answer: {e}")), seeds_gc, usage: None },
        },
        Err(e) => EngineScan { status: "error".into(), at: now, error: Some(e.message), seeds_gc, usage: None },
    }
}

/// The box's engine (Linux with a walled engine only).
pub fn engine_scan(now: i64) -> EngineScan {
    #[cfg(target_os = "linux")]
    {
        if !Path::new(wire::SOCKET).exists() {
            return EngineScan { status: "missing".into(), at: now, ..EngineScan::default() };
        }
        engine_scan_at(Path::new(wire::SOCKET), now)
    }
    #[cfg(not(target_os = "linux"))]
    {
        EngineScan { status: "missing".into(), at: now, ..EngineScan::default() }
    }
}

// ── Postgres ─────────────────────────────────────────────────────────

/// The one query (the helper allows `-d postgres -tA` and stdin only).
pub const PG_SIZES_SQL: &str = "SELECT 'db', datname, pg_database_size(datname) FROM pg_database WHERE datallowconn AND NOT datistemplate\n\
     UNION ALL SELECT 'wal', '', COALESCE((SELECT SUM(size) FROM pg_ls_waldir()), 0)\n\
     UNION ALL SELECT 'dir', current_setting('data_directory'), 0;\n";

/// `psql -tA` rows (`kind|name|bytes`) → sizes.
pub fn parse_pg_sizes(out: &str, now: i64) -> DbScan {
    let mut s = DbScan { at: now, ..DbScan::default() };
    for line in out.lines() {
        let Some((kind, rest)) = line.split_once('|') else { continue };
        let Some((name, n)) = rest.rsplit_once('|') else { continue };
        let bytes: u64 = n.trim().parse().unwrap_or(0);
        match kind {
            "db" => {
                s.databases.insert(name.to_string(), bytes);
            }
            "wal" => s.wal_bytes = bytes,
            "dir" if !name.is_empty() => s.data_directory = Some(name.to_string()),
            _ => {}
        }
    }
    s
}

/// Postgres sizes through the helper (`ops` is the real one in the
/// daemon). `None` when the SQL sidecar isn't running here.
pub fn db_scan_with(ops: &dyn crate::sql::sysops::SystemOps, running: bool, now: i64) -> Option<DbScan> {
    if !running {
        return None;
    }
    Some(match ops.run_helper(&["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-tA"], Some(PG_SIZES_SQL.as_bytes())) {
        Ok(out) => parse_pg_sizes(&out, now),
        Err(e) => DbScan { at: now, error: Some(e), ..DbScan::default() },
    })
}

fn sql_running() -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT status FROM sql_server WHERE id = 1", [], |r| r.get::<_, Option<String>>(0)).ok().flatten().as_deref() == Some("running")
}

// ── memory ───────────────────────────────────────────────────────────

/// `/proc/<pid>/stat` → `(session, rss pages)`.
pub fn parse_proc_stat(stat: &str) -> Option<(i32, u64)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // After the comm: f[0] = state (field 3), f[3] = session (6), f[21] = rss (24).
    Some((f.get(3)?.parse().ok()?, f.get(21)?.parse().ok()?))
}

/// Resident bytes per session id, from one `/proc` pass (Linux).
#[allow(unused_mut)]
pub fn rss_by_session() -> HashMap<i32, u64> {
    let mut out = HashMap::new();
    #[cfg(target_os = "linux")]
    {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        if let Ok(rd) = std::fs::read_dir("/proc") {
            for e in rd.flatten() {
                let name = e.file_name();
                let Some(n) = name.to_str() else { continue };
                if !n.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
                if let Some((sid, pages)) = parse_proc_stat(&stat) {
                    *out.entry(sid).or_insert(0) += pages.saturating_mul(page);
                }
            }
        }
    }
    out
}

/// The agents' memory leaf's `memory.current` (Linux, delegated unit).
pub fn agents_mem_bytes() -> Option<u64> {
    let s = k2_core::agents_leaf::status_json();
    let leaf = s["leaf"].as_str()?;
    std::fs::read_to_string(Path::new(leaf).join("memory.current")).ok()?.trim().parse().ok()
}

// ── the snapshot ─────────────────────────────────────────────────────

/// The paths whose filesystems the guard and the watch track, labelled as
/// the view's `holds` (`k2Home`, `workspaces`, `k2app`, `postgres`, `mail`).
pub fn tracked_paths() -> Vec<(String, PathBuf)> {
    let mut v = vec![("k2Home".to_string(), k2_core::paths::k2_home())];
    let ws: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.prepare("SELECT path FROM projects")
            .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>())
            .unwrap_or_default()
    };
    v.extend(ws.into_iter().filter(|p| !p.trim().is_empty()).map(|p| ("workspaces".to_string(), PathBuf::from(p))));
    if Path::new(K2APP_HOME).exists() {
        v.push(("k2app".into(), PathBuf::from(K2APP_HOME)));
    }
    let pg = cache_now().databases.and_then(|d| d.data_directory).map(PathBuf::from);
    match pg {
        Some(p) => v.push(("postgres".into(), p)),
        None if Path::new(PG_DEFAULT_DIR).exists() => v.push(("postgres".into(), PathBuf::from(PG_DEFAULT_DIR))),
        None => {}
    }
    if Path::new(k2_root_proto::paths::STALWART_DATA_DIR).exists() {
        v.push(("mail".into(), PathBuf::from(k2_root_proto::paths::STALWART_DATA_DIR)));
    }
    v
}

/// A row owner from a maintainer (a parked App or a removed workspace
/// belongs to the server: its share goes to the owner only).
fn owner_of(conn: &Connection, ws: &str, memo: &mut HashMap<String, Owner>) -> Owner {
    memo.entry(ws.to_string())
        .or_insert_with(|| {
            let m = super::coordination::maintainer_with(conn, ws);
            if m.kind == "agent" {
                Owner { kind: "agent".into(), agent: m.agent, handle: m.handle, address: m.address, workspace_id: Some(ws.to_string()) }
            } else {
                Owner::server()
            }
        })
        .clone()
}

/// The fs id holding `p` (memoized per build).
fn disk_of(p: &Path, memo: &mut HashMap<PathBuf, Option<String>>) -> Option<String> {
    memo.entry(p.to_path_buf()).or_insert_with(|| dg::probe(p).map(|f| f.id)).clone()
}

/// What a runtime App row is called in the view.
pub fn path_word(a: &store::AppRow) -> &'static str {
    if a.path == "open" {
        "open"
    } else if a.isolation == "container" {
        "walled"
    } else {
        "process"
    }
}

/// Is this App's folder the engine's (walled or the process exception)?
pub fn engine_measured(a: &store::AppRow) -> bool {
    a.isolation == "container" || a.is_process_exception()
}

/// The rows read from SQLite under one lock.
struct Rows {
    apps: Vec<store::AppRow>,
    app_seeds: BTreeMap<String, u64>,
    budgets: BTreeMap<String, u64>,
    services: Vec<(String, String, String)>,
    projects: Vec<(String, String)>,
    databases: Vec<(String, String)>,
    owners: HashMap<String, Owner>,
    will_clear: Option<(u64, u64)>,
    /// `SUM(size_bytes)` over every stored Seed (the daemon's copies).
    seeds_total: u64,
}

fn read_rows(now: i64, sweeping: bool) -> Rows {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let apps = store::list_apps(&conn, None).unwrap_or_default();
    let mut owners: HashMap<String, Owner> = HashMap::new();
    let mut app_seeds = BTreeMap::new();
    let mut budgets = BTreeMap::new();
    for a in &apps {
        owner_of(&conn, &a.owner_workspace, &mut owners);
        // The daemon's Seed copies this App names (live, preview, deployments).
        let n: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM seeds WHERE hash IN ( \
                   SELECT live_seed FROM apps WHERE app_id = ?1 UNION SELECT preview_seed FROM apps WHERE app_id = ?1 \
                   UNION SELECT to_seed FROM deployments WHERE app_id = ?1 UNION SELECT from_seed FROM deployments WHERE app_id = ?1)",
                rusqlite::params![a.app_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        app_seeds.insert(a.app_id.clone(), n.max(0) as u64);
        if let Some(b) = k2_core::app_grace::config::load(&conn, &a.app_id).ok().and_then(|c| c.resources.disk_bytes) {
            if b > 0 {
                budgets.insert(a.app_id.clone(), b as u64);
            }
        }
    }
    let services: Vec<(String, String, String)> = conn
        .prepare(
            "SELECT id, project_id, name FROM published_services WHERE kind = 'cmd' \
             AND id NOT IN (SELECT service_id FROM apps) ORDER BY name",
        )
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>())
        .unwrap_or_default();
    for (_, ws, _) in &services {
        owner_of(&conn, ws, &mut owners);
    }
    let projects: Vec<(String, String)> = conn
        .prepare("SELECT id, path FROM projects ORDER BY id")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>())
        .unwrap_or_default();
    for (ws, _) in &projects {
        owner_of(&conn, ws, &mut owners);
    }
    let databases: Vec<(String, String)> = conn
        .prepare("SELECT name, project_id FROM sql_databases WHERE status IN ('active', 'test') ORDER BY name")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>())
        .unwrap_or_default();
    for (_, ws) in &databases {
        owner_of(&conn, ws, &mut owners);
    }
    let will_clear = if sweeping {
        store::sweep_candidates(&conn, now, store::SWEEP_MAX_AGE_SECS, store::SWEEP_KEEP_DEPLOYMENTS)
            .map(|v| (v.len() as u64, v.iter().map(|s| s.size_bytes.max(0) as u64).sum()))
            .map_err(|e| k2_core::log_debug!("[disk-scan] Seed sweep preview: {e}"))
            .ok()
    } else {
        None
    };
    let seeds_total = conn.query_row("SELECT COALESCE(SUM(size_bytes), 0) FROM seeds", [], |r| r.get::<_, i64>(0)).unwrap_or(0).max(0) as u64;
    Rows { apps, app_seeds, budgets, services, projects, databases, owners, will_clear, seeds_total }
}

/// Live numbers gathered outside the DB lock.
#[derive(Debug, Clone, Default)]
pub struct Live {
    /// App id → memory.
    pub app_mem: HashMap<String, u64>,
    /// (workspace, service name) → memory.
    pub service_mem: HashMap<(String, String), u64>,
    pub running_services: BTreeSet<(String, String)>,
}

fn gather_live(apps: &[store::AppRow]) -> Live {
    let mut l = Live::default();
    for a in apps {
        if let Some(inst) = super::supervisor::live_instance(&a.app_id) {
            if let Ok(s) = super::supervisor::engine_of(&inst.handle).stats(&inst.handle) {
                if let Some(m) = s.mem_current {
                    l.app_mem.insert(a.app_id.clone(), m);
                }
            }
        }
    }
    let rss = rss_by_session();
    for (ws, name, pid) in crate::publish_runtime::live_children() {
        l.running_services.insert((ws.clone(), name.clone()));
        if let Some(b) = rss.get(&pid) {
            l.service_mem.insert((ws, name), *b);
        }
    }
    l
}

/// Build the view's rows from the cache, SQLite and the live numbers.
pub fn build_snapshot(c: &Cache, now: i64) -> Snapshot {
    let sweeping = k2_core::disk_guard::guard().mode != dg::Mode::Off;
    let rows = read_rows(now, sweeping);
    let live = gather_live(&rows.apps);
    compose(c, &rows, &live, now, sweeping)
}

fn compose(c: &Cache, rows: &Rows, live: &Live, now: i64, sweeping: bool) -> Snapshot {
    let mut disks: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut out: Vec<Consumer> = Vec::new();
    let usage = c.engine.as_ref().and_then(|e| e.usage.as_ref());
    let engine_at = c.engine.as_ref().map(|e| e.at);
    for a in &rows.apps {
        let owner = rows.owners.get(&a.owner_workspace).cloned().unwrap_or_else(Owner::server);
        let seeds = rows.app_seeds.get(&a.app_id).copied().unwrap_or(0);
        let mut parts: BTreeMap<String, u64> = BTreeMap::new();
        let (folder, scanned_at, partial, disk) = if engine_measured(a) {
            let d = disk_of(Path::new(K2APP_HOME), &mut disks);
            match usage.and_then(|u| u.apps.get(&a.app_id)) {
                Some(m) => {
                    parts.insert("data".into(), m.data);
                    parts.insert("cache".into(), m.cache);
                    parts.insert("logs".into(), m.logs);
                    if m.other > 0 {
                        parts.insert("other".into(), m.other);
                    }
                    (Some(m.total()), engine_at, m.partial, d)
                }
                None => (None, engine_at, false, d),
            }
        } else {
            let d = disk_of(&k2_core::deploy::app_dir(&a.app_id), &mut disks);
            match c.apps.get(&a.app_id) {
                Some(f) => {
                    for (k, v) in &f.parts {
                        let key = if ["data", "cache", "logs", "home"].contains(&k.as_str()) { k.as_str() } else { "other" };
                        *parts.entry(key.into()).or_insert(0) += v;
                    }
                    (Some(f.bytes), Some(f.scanned_at), f.partial, d)
                }
                None => (None, None, false, d),
            }
        };
        if seeds > 0 {
            parts.insert("seeds".into(), seeds);
        }
        out.push(Consumer {
            kind: "app".into(),
            id: a.app_id.clone(),
            path: Some(path_word(a).into()),
            state: Some(a.state.clone()),
            owner,
            disk,
            disk_bytes: folder.map(|f| f.saturating_add(seeds)),
            parts,
            budget_bytes: rows.budgets.get(&a.app_id).copied(),
            mem_bytes: live.app_mem.get(&a.app_id).copied(),
            scanned_at,
            partial,
            ..Consumer::default()
        });
    }
    for (_, ws, name) in &rows.services {
        let key = (ws.clone(), name.clone());
        let mut parts = BTreeMap::new();
        if let Ok(m) = std::fs::metadata(crate::publish_runtime::log_path(ws, name)) {
            parts.insert("logs".to_string(), m.len());
        }
        out.push(Consumer {
            kind: "service".into(),
            id: name.clone(),
            walled: Some(false),
            state: Some(if live.running_services.contains(&key) { "running" } else { "stopped" }.into()),
            owner: rows.owners.get(ws).cloned().unwrap_or_else(Owner::server),
            parts,
            mem_bytes: live.service_mem.get(&key).copied(),
            ..Consumer::default()
        });
    }
    let pg = c.databases.as_ref();
    let pg_disk = pg.and_then(|d| d.data_directory.clone()).map(PathBuf::from).and_then(|p| disk_of(&p, &mut disks));
    let mut owned_dbs: BTreeSet<&str> = BTreeSet::new();
    for (name, ws) in &rows.databases {
        owned_dbs.insert(name.as_str());
        out.push(Consumer {
            kind: "database".into(),
            id: name.clone(),
            owner: rows.owners.get(ws).cloned().unwrap_or_else(Owner::server),
            disk: pg_disk.clone(),
            disk_bytes: pg.and_then(|d| d.databases.get(name).copied()),
            scanned_at: pg.map(|d| d.at),
            ..Consumer::default()
        });
    }
    for (ws, path) in &rows.projects {
        let f = c.workspaces.get(ws);
        out.push(Consumer {
            kind: "workspace".into(),
            id: ws.clone(),
            owner: rows.owners.get(ws).cloned().unwrap_or_else(Owner::server),
            disk: disk_of(Path::new(path), &mut disks),
            disk_bytes: f.map(|f| f.bytes),
            parts: f.map(|f| top_parts(&f.parts, WORKSPACE_PARTS)).unwrap_or_default(),
            scanned_at: f.map(|f| f.scanned_at),
            partial: f.is_some_and(|f| f.partial),
            folder: Some(path.clone()),
            ..Consumer::default()
        });
    }
    // Shared parts (the server's).
    if let Some(d) = pg.filter(|d| d.error.is_none()) {
        let unowned: u64 = d.databases.iter().filter(|(n, _)| !owned_dbs.contains(n.as_str())).map(|(_, b)| *b).sum();
        let mut parts = BTreeMap::new();
        parts.insert("wal".to_string(), d.wal_bytes);
        parts.insert("unowned".to_string(), unowned);
        out.push(Consumer {
            kind: "shared".into(),
            id: "postgres".into(),
            owner: Owner::server(),
            disk: pg_disk.clone(),
            disk_bytes: Some(d.wal_bytes.saturating_add(unowned)),
            parts,
            scanned_at: Some(d.at),
            ..Consumer::default()
        });
    }
    if let Some(t) = crate::mail::backup::load_state().latest {
        out.push(Consumer {
            kind: "shared".into(),
            id: "mail".into(),
            owner: Owner::server(),
            disk: disk_of(Path::new(k2_root_proto::paths::STALWART_DATA_DIR), &mut disks),
            disk_bytes: Some(t.store_bytes),
            scanned_at: Some(t.at),
            ..Consumer::default()
        });
    }
    if let Some(k) = &c.k2 {
        let seeds_total = rows.seeds_total;
        let mut parts = k2_parts(&k.parts);
        parts.insert("seeds".into(), seeds_total);
        out.push(Consumer {
            kind: "shared".into(),
            id: "k2".into(),
            owner: Owner::server(),
            disk: disk_of(&k2_core::paths::k2_home(), &mut disks),
            disk_bytes: Some(k.bytes.saturating_add(seeds_total)),
            parts,
            scanned_at: Some(k.scanned_at),
            partial: k.partial,
            ..Consumer::default()
        });
    }
    if let Some(u) = usage {
        let seeds: u64 = u.seeds.values().sum();
        let mut parts = BTreeMap::new();
        let images = u.images.as_ref().map(|i| i.total_bytes);
        if let Some(i) = images {
            parts.insert("images".to_string(), i);
        }
        parts.insert("seeds".to_string(), seeds);
        parts.insert("builds".to_string(), u.builds);
        out.push(Consumer {
            kind: "shared".into(),
            id: "k2appImages".into(),
            owner: Owner::server(),
            disk: disk_of(Path::new(K2APP_HOME), &mut disks),
            disk_bytes: Some(images.unwrap_or(0).saturating_add(seeds).saturating_add(u.builds)),
            unused_bytes: u.images.as_ref().map(|i| i.unused_bytes),
            parts,
            scanned_at: engine_at,
            partial: u.partial,
            ..Consumer::default()
        });
    }
    if !c.tools.is_empty() {
        let parts: BTreeMap<String, u64> = c.tools.iter().map(|(k, f)| (k.clone(), f.bytes)).collect();
        out.push(Consumer {
            kind: "shared".into(),
            id: "agentTools".into(),
            owner: Owner::server(),
            disk: dirs::home_dir().and_then(|h| disk_of(&h, &mut disks)),
            disk_bytes: Some(parts.values().sum()),
            parts,
            scanned_at: c.tools.values().map(|f| f.scanned_at).min(),
            partial: c.tools.values().any(|f| f.partial),
            ..Consumer::default()
        });
    }
    Snapshot {
        consumers: out,
        k2_will_clear: rows.will_clear.map(|(n, b)| WillClear {
            seeds_bytes: b,
            seeds: n,
            next_sweep_at: if sweeping { super::disk_watch::next_sweep_at() } else { None },
        }),
        scan: ScanTimes {
            disk_at: Some(now),
            apps_at: c.apps_at,
            workspaces_at: c.workspaces_at,
            databases_at: c.databases.as_ref().map(|d| d.at),
            running: dg::scan_running(),
            engine: c.engine.as_ref().map(|e| e.status.clone()),
        },
    }
}

fn last_live() -> &'static Mutex<Option<Instant>> {
    static L: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(None))
}

/// Rebuild and publish the snapshot from the cache and the live numbers
/// (memory, states, owners, the 60-second disk time).
pub fn republish() -> Arc<Snapshot> {
    let s = build_snapshot(&cache_now(), k2_core::deploy::now());
    dg::publish_snapshot(s);
    *last_live().lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
    dg::latest_snapshot().expect("just published")
}

/// The view's hook (§5.1 "on request, 10 s cache"): republish when the
/// live numbers are older than `max_age`, else hand back what is there.
pub fn refresh_live_if_older(max_age: Duration) -> Option<Arc<Snapshot>> {
    let stale = last_live().lock().unwrap_or_else(|p| p.into_inner()).is_none_or(|t| t.elapsed() >= max_age);
    if stale && scanner_started() {
        return Some(republish());
    }
    dg::latest_snapshot()
}

/// One App's measured disk and its budget, when someone set a budget
/// (`k2 app config <app> --disk N`). For the per-App budget notices and
/// the guard's `"budget": true` refusal.
pub fn app_over_budget(app_id: &str) -> Option<(u64, u64)> {
    let s = dg::latest_snapshot()?;
    let c = s.consumers.iter().find(|c| c.kind == "app" && c.id == app_id)?;
    Some((c.disk_bytes?, c.budget_bytes?))
}

// ── the sweeps ───────────────────────────────────────────────────────

/// The process-engine Apps' folders, the engine's answer, and `~/.k2`.
pub fn scan_apps(now: i64) {
    let apps = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_apps(&conn, None).unwrap_or_default()
    };
    let deadline = Instant::now() + SWEEP_LIMIT;
    let mut walked = BTreeMap::new();
    for a in apps.iter().filter(|a| !engine_measured(a)) {
        let opts = WalkOpts { skip: &[], deadline: (Instant::now() + FOLDER_LIMIT).min(deadline) };
        if let Some(f) = walk(&k2_core::deploy::app_dir(&a.app_id), &opts, now) {
            walked.insert(a.app_id.clone(), f);
        }
    }
    let engine = if apps.iter().any(engine_measured) || Path::new(K2APP_HOME).exists() { Some(engine_scan(now)) } else { None };
    let home = k2_core::paths::k2_home();
    let skip = [k2_core::deploy::seeds_dir()];
    let k2 = walk(&home, &WalkOpts { skip: &skip, deadline: Instant::now() + FOLDER_LIMIT }, now);
    update_cache(|c| {
        c.apps = walked;
        c.apps_at = Some(now);
        if engine.is_some() {
            c.engine = engine;
        }
        c.k2 = k2;
    });
}

/// Postgres sizes (hourly; the sudo log is why it isn't every 30 min).
pub fn scan_databases(now: i64) {
    let scan = db_scan_with(&crate::sql::sysops::RealSystemOps, cfg!(target_os = "linux") && sql_running(), now);
    update_cache(|c| c.databases = scan);
}

/// The workspace folders and the agent tools (every 6 h, and a rescan).
/// Order (§5.2): the partial and never-scanned first, then workspaces
/// with Apps, then by last size, biggest first.
pub fn scan_folders(now: i64) {
    let (projects, with_apps): (Vec<(String, String)>, BTreeSet<String>) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let p = conn
            .prepare("SELECT id, path FROM projects")
            .and_then(|mut st| st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>())
            .unwrap_or_default();
        let a = store::list_apps(&conn, None).unwrap_or_default().into_iter().map(|a| a.owner_workspace).collect();
        (p, a)
    };
    let prev = cache_now().workspaces;
    let order = folder_order(&projects, &with_apps, &prev);
    let roots: Vec<PathBuf> = projects.iter().map(|(_, p)| PathBuf::from(p)).collect();
    let home = k2_core::paths::k2_home();
    let deadline = Instant::now() + SWEEP_LIMIT;
    let mut next = prev.clone();
    next.retain(|ws, _| projects.iter().any(|(id, _)| id == ws));
    for (ws, path) in order {
        let root = PathBuf::from(&path);
        if Instant::now() >= deadline {
            // Out of time: keep the last numbers; the next sweep starts here.
            if let Some(f) = next.get_mut(&ws) {
                f.partial = true;
            }
            continue;
        }
        // Each byte once: a nested workspace and K2's home are their own rows.
        let mut skip: Vec<PathBuf> = roots.iter().filter(|r| **r != root && r.starts_with(&root)).cloned().collect();
        if home.starts_with(&root) {
            skip.push(home.clone());
        }
        let opts = WalkOpts { skip: &skip, deadline: (Instant::now() + FOLDER_LIMIT).min(deadline) };
        match walk(&root, &opts, now) {
            Some(mut f) => {
                f.parts = top_parts(&f.parts, 10);
                next.insert(ws, f);
            }
            None => {
                next.remove(&ws);
            }
        }
    }
    let mut tools = BTreeMap::new();
    if let Some(h) = dirs::home_dir() {
        for d in TOOL_DIRS {
            if Instant::now() >= deadline {
                break;
            }
            let opts = WalkOpts { skip: &[], deadline: (Instant::now() + FOLDER_LIMIT).min(deadline) };
            if let Some(f) = walk(&h.join(d), &opts, now) {
                tools.insert((*d).to_string(), FolderScan { parts: BTreeMap::new(), ..f });
            }
        }
    }
    update_cache(|c| {
        c.workspaces = next;
        c.workspaces_at = Some(now);
        c.tools = tools;
    });
}

/// The order the folder sweep walks workspaces in.
pub fn folder_order(projects: &[(String, String)], with_apps: &BTreeSet<String>, prev: &BTreeMap<String, FolderScan>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = projects.iter().filter(|(_, p)| !p.trim().is_empty()).cloned().collect();
    v.sort_by(|(a, _), (b, _)| {
        let key = |ws: &String| {
            let f = prev.get(ws);
            let first = f.is_none_or(|f| f.partial);
            (!first, !with_apps.contains(ws), std::cmp::Reverse(f.map(|f| f.bytes).unwrap_or(0)))
        };
        key(a).cmp(&key(b)).then_with(|| a.cmp(b))
    });
    v
}

/// One full sweep (a rescan, or the 6-hour one): Apps, `~/.k2`, folders,
/// tools; Postgres when due. Brackets itself for the 5-minute rule.
pub fn full_sweep(db_due: bool) {
    dg::set_scan_running(true, None);
    let now = k2_core::deploy::now();
    scan_apps(now);
    republish();
    if db_due {
        scan_databases(now);
    }
    scan_folders(now);
    persist();
    republish();
    dg::set_scan_running(false, Some(k2_core::deploy::now()));
}

/// Block until the daemon's shared DB is open. `db::shared()` before
/// `init_database` panics in a release build and, in a `test-util` build
/// (the daemon binary integration tests run), silently opens a private
/// in-memory DB for the whole process: never call it early.
pub fn wait_for_db() {
    while k2_core::db::try_shared().is_none() {
        std::thread::sleep(Duration::from_millis(500));
    }
}

static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn scanner_started() -> bool {
    STARTED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The scanner thread (spawned by `disk_watch::spawn`, every platform the
/// daemon runs on; headless).
pub fn spawn() {
    if cfg!(test) || STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("disk-scan".into()).spawn(|| {
        lower_priority();
        wait_for_db();
        dg::register_scanner();
        *cache().lock().unwrap_or_else(|p| p.into_inner()) = load_cache_file(&cache_path());
        republish();
        let boot = k2_core::deploy::now();
        // The last Postgres try (a box without the SQL sidecar has no
        // DbScan to date it by).
        let mut db_tried: Option<i64> = None;
        loop {
            let c = cache_now();
            let now = k2_core::deploy::now();
            let first = boot + BOOT_DELAY_SECS;
            let apps_next = c.apps_at.map(|t| t + APPS_EVERY_SECS).unwrap_or(first).max(first);
            let db_last = c.databases.as_ref().map(|d| d.at).max(db_tried);
            let db_next = db_last.map(|t| t + DATABASES_EVERY_SECS).unwrap_or(first).max(first);
            let full_next = c.workspaces_at.map(|t| t + FOLDERS_EVERY_SECS).unwrap_or(first + 180).max(first);
            let next = apps_next.min(db_next).min(full_next);
            let wait = Duration::from_secs((next - now).clamp(1, FOLDERS_EVERY_SECS) as u64);
            let asked = dg::scanner_wait(wait);
            let now = k2_core::deploy::now();
            if now >= db_next {
                db_tried = Some(now);
            }
            let res = std::panic::catch_unwind(|| {
                if asked || now >= full_next {
                    full_sweep(asked || now >= db_next);
                } else {
                    dg::set_scan_running(true, None);
                    if now >= apps_next {
                        scan_apps(now);
                    }
                    if now >= db_next {
                        scan_databases(now);
                    }
                    persist();
                    republish();
                    dg::set_scan_running(false, None);
                }
            });
            if res.is_err() {
                dg::set_scan_running(false, None);
                k2_core::log_debug!("[disk-scan] a sweep panicked; the next one runs on schedule");
            }
        }
    });
}

#[cfg(test)]
#[path = "disk_scan_tests.rs"]
mod tests;
