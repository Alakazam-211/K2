//! The disk watch and the Seed sweep (prd-app-resources-disk-guard-v1
//! §9–§10, Slices A4 and A5, lane M). Headless: one daemon thread, every
//! platform, no client needed.
//!
//! - **Every 60 s**: republish the box usage (fresh memory and disk time),
//!   probe each tracked filesystem, and on a **crossing up** (ok → warn →
//!   refuse → critical) send notices **once**. A line re-arms only after
//!   the fs drops 5 points below it **and** has 5 GiB more free than it
//!   ([`k2_core::disk_guard::rearmed`]). State lives in
//!   `~/.k2/disk-watch.json`, so a restart never repeats a notice.
//! - **Who hears** (§10.2): the owner gets one ticket per crossing
//!   (`file_owner_notice`: Owner and Admin logins, with a phone push;
//!   priority 2 at warn, 1 at refuse and critical, "CRITICAL" in the
//!   title at critical). The agents behind the three biggest owners on
//!   that disk get an inbox note at warn and a `k2 msg` wake at refuse and
//!   critical. One message per agent per cause per 24 h. Growth (an App,
//!   database or folder up 5 GB, or 20% and 1 GB, in a day) and per-App
//!   budgets (80% note, 100% wake) go to the owning agent. A parked App's
//!   or removed workspace's share goes to the owner only.
//! - **Fresh owners first**: on a crossing, if the folder scan is over an
//!   hour old, the watch asks for a rescan and waits up to 10 minutes so
//!   the notice names the right agents.
//! - **The Seed sweep** (§9), daily, ~10 minutes after boot: K2's own old,
//!   unused Seeds, both copies. Picked under the DB lock, each detached
//!   (one rename) under it, every tree deleted outside it. The engine copy
//!   through `seeds-gc` only when the engine lists it: the swept hashes
//!   plus engine-only orphans, never a kept one. **Nothing else is ever
//!   deleted**: no App data, database, mail, workspace, upload, transcript,
//!   backup, Trash or image.
//! - Mode `off`: no notices, no sweep (the view still reads).
//!
//! Notices go through one [`NoticeSink`]; tests use a recording one, so no
//! test files a real ticket or wakes a real agent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use k2_core::deploy::store;
use k2_core::disk_guard::{self as dg, human_gb, human_gib, Fs, Level, Lines, Snapshot};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// `~/.k2/disk-watch.json`.
pub const STATE_FILE: &str = "disk-watch.json";
pub const TICK: Duration = Duration::from_secs(60);
/// One message per agent per cause per day.
pub const DEDUPE_SECS: i64 = 86_400;
/// A crossing first refreshes owners older than this…
pub const FRESH_OWNERS_SECS: i64 = 3600;
/// …and waits this long for the rescan.
pub const FRESH_WAIT: Duration = Duration::from_secs(600);
/// The Seed sweep: first run after boot, then every day.
pub const SWEEP_FIRST_SECS: i64 = 600;
pub const SWEEP_EVERY_SECS: i64 = 86_400;
/// Growth in a day (§10.2): more than 5 GB, or 20% and at least 1 GB.
pub const GROWTH_BYTES: u64 = 5_000_000_000;
pub const GROWTH_PCT: u64 = 20;
pub const GROWTH_MIN_BYTES: u64 = 1_000_000_000;
/// The per-App budget's first notice.
pub const BUDGET_NOTE_PCT: u64 = 80;
/// How many owners hear about a crossing.
pub const TOP_OWNERS: usize = 3;
/// The ticket's "agent" name.
const NOTICE_FROM: &str = "k2-disk";

// ── state ────────────────────────────────────────────────────────────

/// Yesterday's totals for the growth notice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GrowthBase {
    pub at: i64,
    /// `app:<id>` / `database:<name>` / `workspace:<id>` → bytes.
    pub totals: BTreeMap<String, u64>,
}

/// `~/.k2/disk-watch.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchState {
    /// Per fs id: the highest line crossed and not yet re-armed.
    pub levels: BTreeMap<String, Level>,
    /// `<workspace>|<cause>` → when the last message went.
    pub sent: BTreeMap<String, i64>,
    pub growth: Option<GrowthBase>,
    pub last_sweep_at: Option<i64>,
}

pub fn state_path() -> PathBuf {
    k2_core::paths::k2_home().join(STATE_FILE)
}

pub fn load_state(path: &Path) -> WatchState {
    match std::fs::read_to_string(path) {
        Err(_) => WatchState::default(),
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            k2_core::log_debug!("[disk-watch] {} unreadable ({e}); starting fresh", path.display());
            WatchState::default()
        }),
    }
}

/// Atomic (temp + rename). A failed save is logged: the next tick tries again.
pub fn save_state(path: &Path, s: &WatchState) -> Result<(), String> {
    let dir = path.parent().ok_or("state path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{STATE_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))
}

// ── notices ──────────────────────────────────────────────────────────

/// One message the watch sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// An `fyi` ticket for the server's people (1 = most urgent).
    Owner { title: String, body: String, priority: i64 },
    /// An inbox note to a workspace's agent (no wake).
    Note { workspace: String, title: String, text: String },
    /// A `k2 msg` wake of a workspace's agent.
    Wake { workspace: String, text: String },
}

/// Where notices go. The daemon's is [`LiveSink`]; tests record.
pub trait NoticeSink: Send + Sync {
    fn owner_ticket(&self, title: &str, body: &str, priority: i64);
    fn agent_note(&self, workspace: &str, title: &str, text: &str);
    fn agent_wake(&self, workspace: &str, text: &str);
}

/// The real rails: `file_owner_notice` (ticket + push) and the App
/// alerts (`alerts::note` / `alerts::notify`).
pub struct LiveSink;

impl NoticeSink for LiveSink {
    fn owner_ticket(&self, title: &str, body: &str, priority: i64) {
        match crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], NOTICE_FROM, title, body) {
            Ok(item) if priority != item.priority => {
                // `file_owner_notice` files at 2; refuse and critical go to 1.
                let db = k2_core::db::shared();
                let conn = db.lock();
                if let Err(e) = conn.execute("UPDATE feedback SET priority = ?1 WHERE id = ?2", rusqlite::params![priority, item.id]) {
                    k2_core::log_debug!("[disk-watch] ticket priority: {e}");
                }
            }
            Ok(_) => {}
            Err(e) => k2_core::log_debug!("[disk-watch] owner ticket: {e}"),
        }
    }
    fn agent_note(&self, workspace: &str, title: &str, text: &str) {
        super::alerts::note(workspace, title, text);
    }
    fn agent_wake(&self, workspace: &str, text: &str) {
        super::alerts::notify(workspace, text);
    }
}

static SINK: RwLock<Option<Arc<dyn NoticeSink>>> = RwLock::new(None);

/// Replace the daemon's sink (integration tests). `None` = the real one.
pub fn set_sink(s: Option<Arc<dyn NoticeSink>>) {
    *SINK.write().unwrap_or_else(|e| e.into_inner()) = s;
}

fn live_sink() -> Arc<dyn NoticeSink> {
    SINK.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Arc::new(LiveSink))
}

/// Send `notices` through `sink`, one guard-log line each.
pub fn deliver(notices: &[Notice], sink: &dyn NoticeSink) {
    for n in notices {
        match n {
            Notice::Owner { title, body, priority } => {
                sink.owner_ticket(title, body, *priority);
                dg::log_event("notice", json!({ "to": "owner", "title": title, "priority": priority }));
            }
            Notice::Note { workspace, title, text } => {
                sink.agent_note(workspace, title, text);
                dg::log_event("notice", json!({ "to": "agent", "workspace": workspace, "how": "note", "title": title }));
            }
            Notice::Wake { workspace, text } => {
                sink.agent_wake(workspace, text);
                dg::log_event("notice", json!({ "to": "agent", "workspace": workspace, "how": "wake" }));
            }
        }
    }
}

/// Once per `DEDUPE_SECS` per (workspace, cause): `true` = send (and mark).
fn may_send(state: &mut WatchState, ws: &str, cause: &str, now: i64) -> bool {
    let key = format!("{ws}|{cause}");
    if state.sent.get(&key).is_some_and(|at| now - at < DEDUPE_SECS) {
        return false;
    }
    state.sent.insert(key, now);
    true
}

/// Drop dedupe marks older than a day (the file stays small).
fn prune(state: &mut WatchState, now: i64) {
    state.sent.retain(|_, at| now - *at < DEDUPE_SECS);
}

// ── crossings ────────────────────────────────────────────────────────

fn lower(l: Level) -> Level {
    match l {
        Level::Critical => Level::Refuse,
        Level::Refuse => Level::Warn,
        Level::Warn | Level::Ok => Level::Ok,
    }
}

/// Update each fs's recorded level and return the crossings up (each fs
/// at most once, at its new level). A recorded line drops only once it
/// re-arms; a fs that disappears keeps its record.
pub fn crossings(state: &mut WatchState, disks: &[Fs], lines: &Lines) -> Vec<(Fs, Level)> {
    let mut up = Vec::new();
    for fs in disks {
        let cur = fs.level(lines);
        let mut rec = state.levels.get(&fs.id).copied().unwrap_or(Level::Ok);
        while rec > cur {
            let (pct, free) = dg::line_of(lines, rec).expect("a level above ok has a line");
            if dg::rearmed(fs, pct, free) {
                rec = lower(rec);
            } else {
                break;
            }
        }
        if cur > rec {
            up.push((fs.clone(), cur));
            rec = cur;
        }
        state.levels.insert(fs.id.clone(), rec);
    }
    up
}

/// An agent's share of the rows on one disk (or every row when no row
/// names a disk).
/// One of an agent's rows: (words, bytes, its biggest parts).
type ShareRow = (String, u64, Vec<(String, u64)>);

#[derive(Debug, Clone, Default)]
struct Share {
    workspace: String,
    handle: String,
    total: u64,
    /// `(words, bytes)`: "your folder", "jobs_db", "App jobs".
    rows: Vec<ShareRow>,
}

fn shares(snap: &Snapshot, fs_id: &str) -> Vec<Share> {
    let on_disk = snap.consumers.iter().any(|c| c.disk.as_deref() == Some(fs_id));
    let mut by: BTreeMap<String, Share> = BTreeMap::new();
    for c in &snap.consumers {
        if on_disk && c.disk.as_deref() != Some(fs_id) {
            continue;
        }
        let (Some(ws), Some(b)) = (c.owner.workspace_id.clone(), c.disk_bytes) else { continue };
        if !c.owner.is_agent() || b == 0 {
            continue;
        }
        let s = by.entry(ws.clone()).or_insert_with(|| Share {
            workspace: ws.clone(),
            handle: c.owner.handle.clone().unwrap_or_else(|| ws.clone()),
            ..Share::default()
        });
        s.total = s.total.saturating_add(b);
        let words = match c.kind.as_str() {
            "workspace" => "your folder".to_string(),
            "app" => format!("App {}", c.id),
            "database" => c.id.clone(),
            other => format!("{other} {}", c.id),
        };
        let parts: Vec<(String, u64)> = if c.kind == "workspace" {
            super::disk_scan::top_parts(&c.parts, 3).into_iter().collect()
        } else {
            Vec::new()
        };
        s.rows.push((words, b, parts));
    }
    let mut v: Vec<Share> = by.into_values().collect();
    for s in &mut v {
        s.rows.sort_by_key(|r| std::cmp::Reverse(r.1));
    }
    v.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.handle.cmp(&b.handle)));
    v
}

fn parts_words(parts: &[(String, u64)]) -> String {
    let mut p: Vec<&(String, u64)> = parts.iter().collect();
    p.sort_by_key(|r| std::cmp::Reverse(r.1));
    p.iter().map(|(n, b)| format!("{n} {}", human_gb(*b))).collect::<Vec<_>>().join(", ")
}

/// "your folder 38.4 GB (uploads 29 GB, …), jobs_db 58.2 GB, App jobs 6.2 GB"
fn rows_words(s: &Share) -> String {
    s.rows
        .iter()
        .map(|(w, b, parts)| if parts.is_empty() { format!("{w} {}", human_gb(*b)) } else { format!("{w} {} ({})", human_gb(*b), parts_words(parts)) })
        .collect::<Vec<_>>()
        .join(", ")
}

/// "agent-nsi 104 GB (its folder 38 GB, jobs_db 58 GB)"
fn owner_words(s: &Share) -> String {
    let top: Vec<String> = s.rows.iter().take(2).map(|(w, b, _)| format!("{} {}", w.replace("your folder", "its folder"), human_gb(*b))).collect();
    format!("{} {} ({})", s.handle, human_gb(s.total), top.join(", "))
}

/// Everything one crossing sends (the caller dedupes agents).
pub fn crossing_notices(
    fs: &Fs,
    level: Level,
    guard: &dg::Guard,
    snap: Option<&Snapshot>,
    host: &str,
    state: &mut WatchState,
    now: i64,
) -> Vec<Notice> {
    let lines = &guard.lines;
    let mount = fs.label();
    let pct = fs.used_pct;
    let free = human_gb(fs.free_bytes);
    let empty = Snapshot::default();
    let snap = snap.unwrap_or(&empty);
    let top: Vec<Share> = shares(snap, &fs.id).into_iter().take(TOP_OWNERS).collect();
    let warn_only = guard.mode == dg::Mode::Warn;
    let mut out = Vec::new();
    // The owner's ticket.
    let title = match level {
        Level::Critical => format!("CRITICAL: disk {mount} {pct}% full on {host} ({free} free)"),
        _ => format!("Disk {mount} {pct}% full on {host} ({free} free)"),
    };
    let mut body = String::new();
    if top.is_empty() {
        body.push_str("K2 doesn't know yet which agents use the most (the folder scan hasn't finished). ");
    } else {
        body.push_str(&format!("The biggest users: {}. K2 has told those agents. ", top.iter().map(owner_words).collect::<Vec<_>>().join(", ")));
    }
    body.push_str("Nothing was deleted. ");
    if let Some(w) = snap.k2_will_clear.as_ref().filter(|w| w.seeds_bytes > 0) {
        body.push_str(&format!("K2 clears {} of its own old Seeds in its next daily sweep. ", human_gb(w.seeds_bytes)));
    }
    match (level, warn_only) {
        (_, true) => body.push_str(&format!(
            "The disk guard is in warn mode: nothing is refused (on, it would stop new deploys at {}% full or {} left). ",
            lines.refuse_pct,
            human_gib(lines.reserve_bytes)
        )),
        (Level::Warn, false) => body.push_str(&format!(
            "New deploys stop at {}% full or {} left. ",
            lines.refuse_pct,
            human_gib(lines.reserve_bytes)
        )),
        (Level::Refuse, false) => body.push_str(&format!(
            "New deploys, builds and large uploads are refused now: this box keeps {} free for Postgres, mail and K2. Running Apps are untouched; restarts and rollbacks still work. You can push one action through with --disk-override \"<reason>\". ",
            human_gib(lines.reserve_bytes)
        )),
        (_, false) => body.push_str(
            "New work is refused, and every override waits for you in person. Running Apps are untouched; restarts and rollbacks still work. ",
        ),
    }
    body.push_str(&format!(
        "See `k2 app resources`. Change the lines: `k2 app resources settings --refuse {} --reserve {}`. Turn the guard off: `k2 app resources settings --mode off`.",
        lines.refuse_pct,
        human_gib(lines.reserve_bytes).replace(" GiB", "G").replace(" MiB", "M")
    ));
    out.push(Notice::Owner { title, body, priority: if level >= Level::Refuse { 1 } else { 2 } });
    // The agents behind the biggest owners.
    for s in &top {
        if !may_send(state, &s.workspace, &format!("disk:{}:{}", fs.id, level.as_str()), now) {
            continue;
        }
        let head = format!("Disk {mount} on this box is {pct}% full ({free} free).");
        let mut text = format!(" You use {}: {}. K2 deletes nothing of yours.", human_gb(s.total), rows_words(s));
        match (level, warn_only) {
            (Level::Warn, _) | (_, true) => text.push_str(&format!(
                " If you can, clear old builds or caches. New deploys stop at {}% full. `k2 app resources --json` shows the whole box.",
                lines.refuse_pct
            )),
            (Level::Refuse, false) => text.push_str(
                " New deploys, builds and large uploads are refused until space is freed. Please clear old builds, caches or uploads you no longer need, then check `k2 app resources --json`.",
            ),
            (_, false) => text.push_str(
                " The disk is critical: new work waits for the server's owner in person. Please free space now (old builds, caches, uploads you no longer need) and check `k2 app resources --json`.",
            ),
        }
        if level == Level::Warn {
            out.push(Notice::Note { workspace: s.workspace.clone(), title: head, text: text.trim().to_string() });
        } else {
            out.push(Notice::Wake { workspace: s.workspace.clone(), text: format!("{head}{text}") });
        }
    }
    out
}

/// The growth notices (§10.2): compares today with the stored base once a
/// day, then makes today the base.
pub fn growth_notices(state: &mut WatchState, snap: &Snapshot, now: i64) -> Vec<Notice> {
    let totals: BTreeMap<String, (u64, Option<String>, String)> = snap
        .consumers
        .iter()
        .filter(|c| matches!(c.kind.as_str(), "app" | "database" | "workspace"))
        .filter_map(|c| {
            let b = c.disk_bytes?;
            let ws = c.owner.is_agent().then(|| c.owner.workspace_id.clone()).flatten();
            let words = match c.kind.as_str() {
                "app" => format!("App {}", c.id),
                "database" => format!("database {}", c.id),
                _ => "your folder".into(),
            };
            Some((format!("{}:{}", c.kind, c.id), (b, ws, words)))
        })
        .collect();
    let Some(base) = state.growth.clone() else {
        state.growth = Some(GrowthBase { at: now, totals: totals.iter().map(|(k, v)| (k.clone(), v.0)).collect() });
        return Vec::new();
    };
    if now - base.at < DEDUPE_SECS {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (key, (b, ws, words)) in &totals {
        let Some(before) = base.totals.get(key).copied() else { continue };
        let grew = b.saturating_sub(before);
        let big = grew > GROWTH_BYTES || (grew >= GROWTH_MIN_BYTES && grew.saturating_mul(100) >= before.saturating_mul(GROWTH_PCT));
        let Some(ws) = ws else { continue };
        if !big || !may_send(state, ws, &format!("growth:{key}"), now) {
            continue;
        }
        out.push(Notice::Note {
            workspace: ws.clone(),
            title: format!("{words} grew {} in a day", human_gb(grew)),
            text: format!(
                "{} grew from {} to {} since yesterday. K2 deletes nothing of yours; if it isn't expected, look for a runaway log, cache or upload. `k2 app resources --json` shows the whole box.",
                capitalize(words),
                human_gb(before),
                human_gb(*b)
            ),
        });
    }
    state.growth = Some(GrowthBase { at: now, totals: totals.iter().map(|(k, v)| (k.clone(), v.0)).collect() });
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Per-App budgets (only Apps someone gave `--disk N`): 80% a note, 100%
/// a wake.
pub fn budget_notices(state: &mut WatchState, snap: &Snapshot, now: i64) -> Vec<Notice> {
    let mut out = Vec::new();
    for c in snap.consumers.iter().filter(|c| c.kind == "app") {
        let (Some(used), Some(budget)) = (c.disk_bytes, c.budget_bytes) else { continue };
        let Some(ws) = c.owner.workspace_id.clone().filter(|_| c.owner.is_agent()) else { continue };
        if budget == 0 {
            continue;
        }
        let pct = used.saturating_mul(100) / budget;
        if pct >= 100 {
            if may_send(state, &ws, &format!("budget100:{}", c.id), now) {
                out.push(Notice::Wake {
                    workspace: ws,
                    text: format!(
                        "App {} uses {} of its {} disk budget. Its new deploys are refused (disk_low, budget) until it is under it: clear its data or cache, or ask the owner for a bigger budget (k2 app config {} --disk N).",
                        c.id,
                        human_gb(used),
                        human_gb(budget),
                        c.id
                    ),
                });
            }
        } else if pct >= BUDGET_NOTE_PCT && may_send(state, &ws, &format!("budget80:{}", c.id), now) {
            out.push(Notice::Note {
                workspace: ws,
                title: format!("App {} is at {pct}% of its disk budget", c.id),
                text: format!(
                    "App {} uses {} of its {} disk budget. At 100% its new deploys are refused. `k2 app inspect {} --resources` shows its parts.",
                    c.id,
                    human_gb(used),
                    human_gb(budget),
                    c.id
                ),
            });
        }
    }
    out
}

/// One watch tick, with every input given (the live thread and the tests
/// both run this). `fresh` refreshes the owner list before a crossing's
/// notices when the folder scan is old (it may block up to 10 minutes).
#[allow(clippy::too_many_arguments)]
pub fn tick_with(
    state: &mut WatchState,
    disks: &[Fs],
    guard: &dg::Guard,
    snap: Option<Arc<Snapshot>>,
    host: &str,
    now: i64,
    sink: &dyn NoticeSink,
    fresh: &dyn Fn() -> Option<Arc<Snapshot>>,
) -> Vec<Notice> {
    if guard.mode == dg::Mode::Off {
        return Vec::new();
    }
    prune(state, now);
    let up = crossings(state, disks, &guard.lines);
    let mut snap = snap;
    let mut out = Vec::new();
    if !up.is_empty() {
        let old = snap.as_ref().and_then(|s| s.scan.workspaces_at).is_none_or(|t| now - t > FRESH_OWNERS_SECS);
        if old {
            if let Some(s) = fresh() {
                snap = Some(s);
            }
        }
        for (fs, level) in &up {
            dg::log_event("crossing", json!({ "disk": fs.id, "mount": fs.label(), "level": level, "usedPct": fs.used_pct, "freeBytes": fs.free_bytes }));
            out.extend(crossing_notices(fs, *level, guard, snap.as_deref(), host, state, now));
        }
    }
    if let Some(s) = snap.as_deref() {
        out.extend(growth_notices(state, s, now));
        out.extend(budget_notices(state, s, now));
    }
    deliver(&out, sink);
    out
}

/// The live freshness step: ask for a rescan and wait (up to 10 min) for
/// the folder scan to finish.
fn fresh_owners() -> Option<Arc<Snapshot>> {
    let before = dg::latest_snapshot().and_then(|s| s.scan.workspaces_at);
    match dg::request_rescan(k2_core::deploy::now()) {
        dg::Rescan::NoScanner => return None,
        dg::Rescan::Skipped(_) => return Some(super::disk_scan::republish()),
        dg::Rescan::Started | dg::Rescan::Running => {}
    }
    let until = std::time::Instant::now() + FRESH_WAIT;
    while std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_secs(5));
        let s = dg::latest_snapshot();
        if s.as_ref().and_then(|s| s.scan.workspaces_at) != before && !dg::scan_running() {
            return s;
        }
    }
    dg::latest_snapshot()
}

// ── the Seed sweep ───────────────────────────────────────────────────

/// The engine side of the sweep (the live one talks to the walled engine;
/// tests record).
pub trait EngineSeeds {
    /// The engine lists `seeds-gc`.
    fn can_gc(&self) -> bool;
    /// The engine's Seed copies (from the latest `disk-usage`).
    fn hashes(&self) -> Vec<String>;
    fn gc(&self, remove: &[String]) -> Result<k2_root_proto::engine::SeedsGcAnswer, String>;
}

/// The walled engine, through the scanner's last answer and the socket.
pub struct LiveEngine;

impl EngineSeeds for LiveEngine {
    fn can_gc(&self) -> bool {
        cfg!(target_os = "linux") && super::disk_scan::cache_now().engine.is_some_and(|e| e.seeds_gc)
    }
    fn hashes(&self) -> Vec<String> {
        super::disk_scan::cache_now().engine.and_then(|e| e.usage).map(|u| u.seeds.into_keys().collect()).unwrap_or_default()
    }
    fn gc(&self, remove: &[String]) -> Result<k2_root_proto::engine::SeedsGcAnswer, String> {
        let a = super::guarded::call(&k2_root_proto::engine::Request::SeedsGc { remove: remove.to_vec() }, None, Duration::from_secs(600))
            .map_err(|e| e.message)?;
        serde_json::from_value(a).map_err(|e| format!("unreadable seeds-gc answer: {e}"))
    }
}

/// What one sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepReport {
    pub seeds: Vec<String>,
    pub bytes: u64,
    pub engine_removed: Vec<String>,
    pub engine_skipped: Vec<String>,
    pub engine_freed_bytes: u64,
    pub engine_error: Option<String>,
    pub error: Option<String>,
}

/// The daily sweep (§9.2). The shared DB lock is held only for the pick
/// and for each detach (a rename and a row delete); every tree is deleted
/// with the lock released.
pub fn run_sweep(now: i64, engine: &dyn EngineSeeds) -> SweepReport {
    let mut r = SweepReport::default();
    let picks = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::sweep_candidates(&conn, now, store::SWEEP_MAX_AGE_SECS, store::SWEEP_KEEP_DEPLOYMENTS)
    };
    let picks = match picks {
        Ok(p) => p,
        Err(e) => {
            r.error = Some(format!("pick: {e}"));
            dg::log_event("sweep", json!({ "error": r.error }));
            return r;
        }
    };
    for s in picks {
        let aside = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::detach_seed(&conn, &s.hash, now, store::SWEEP_MAX_AGE_SECS, store::SWEEP_KEEP_DEPLOYMENTS)
        };
        match aside {
            Ok(Some(p)) => {
                k2_core::deploy::seed::remove_detached(&p);
                r.bytes = r.bytes.saturating_add(s.size_bytes.max(0) as u64);
                r.seeds.push(s.hash);
            }
            Ok(None) => {}
            Err(e) => {
                r.error = Some(format!("detach {}: {e}", s.hash));
                break;
            }
        }
    }
    if engine.can_gc() {
        // The swept hashes, plus engine-only copies no daemon row or kept
        // reference names (orphans).
        let (rows, keep) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let rows: std::collections::BTreeSet<String> =
                store::list_seeds(&conn).map(|v| v.into_iter().map(|s| s.hash).collect()).unwrap_or_default();
            (rows, store::referenced_seeds(&conn, store::SWEEP_KEEP_DEPLOYMENTS))
        };
        match keep {
            Err(e) => r.engine_error = Some(format!("keep set: {e}")),
            Ok(keep) => {
                let mut list: Vec<String> = r.seeds.clone();
                for h in engine.hashes() {
                    if !rows.contains(&h) && !keep.contains(&h) && !list.contains(&h) {
                        list.push(h);
                    }
                }
                list.retain(|h| !keep.contains(h));
                if !list.is_empty() {
                    match engine.gc(&list) {
                        Ok(a) => {
                            r.engine_removed = a.removed;
                            r.engine_skipped = a.skipped;
                            r.engine_freed_bytes = a.freed_bytes;
                        }
                        Err(e) => r.engine_error = Some(e),
                    }
                }
            }
        }
    }
    dg::log_event(
        "sweep",
        json!({
            "seeds": r.seeds.len(), "bytes": r.bytes, "hashes": r.seeds,
            "engine": { "removed": r.engine_removed, "skipped": r.engine_skipped, "freedBytes": r.engine_freed_bytes, "error": r.engine_error },
            "error": r.error,
        }),
    );
    r
}

static NEXT_SWEEP_AT: AtomicI64 = AtomicI64::new(0);
static SWEEPING: AtomicBool = AtomicBool::new(false);

/// When the next daily sweep runs (`None` before the watch started).
pub fn next_sweep_at() -> Option<i64> {
    match NEXT_SWEEP_AT.load(Ordering::Relaxed) {
        0 => None,
        t => Some(t),
    }
}

fn sweep_in_background(now: i64) {
    if SWEEPING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("seed-sweep".into()).spawn(move || {
        let r = std::panic::catch_unwind(|| run_sweep(now, &LiveEngine));
        if r.is_err() {
            dg::log_event("sweep", json!({ "error": "the sweep panicked" }));
        }
        SWEEPING.store(false, Ordering::SeqCst);
        super::disk_scan::republish();
    });
}

// ── the thread ───────────────────────────────────────────────────────

fn host() -> String {
    super::coordination::this_host().unwrap_or_else(|| "this server".into())
}

/// Boot: the scanner and the watch (one call in `daemon_main`). Every
/// platform; headless.
pub fn spawn() {
    if cfg!(test) {
        return;
    }
    static ONCE: OnceLock<()> = OnceLock::new();
    if ONCE.set(()).is_err() {
        return;
    }
    super::disk_scan::spawn();
    let _ = std::thread::Builder::new().name("disk-watch".into()).spawn(|| {
        super::disk_scan::wait_for_db();
        let path = state_path();
        let mut state = load_state(&path);
        let boot = k2_core::deploy::now();
        NEXT_SWEEP_AT.store(boot + SWEEP_FIRST_SECS, Ordering::Relaxed);
        loop {
            std::thread::sleep(TICK);
            let now = k2_core::deploy::now();
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let g = dg::guard();
                let snap = super::disk_scan::republish();
                if g.mode == dg::Mode::Off {
                    return;
                }
                let disks: Vec<Fs> = dg::group(&super::disk_scan::tracked_paths()).into_iter().map(|(f, _)| f).collect();
                tick_with(&mut state, &disks, &g, Some(snap), &host(), now, &*live_sink(), &fresh_owners);
                if let Err(e) = save_state(&path, &state) {
                    k2_core::log_debug!("[disk-watch] state: {e}");
                }
                if now >= NEXT_SWEEP_AT.load(Ordering::Relaxed) {
                    NEXT_SWEEP_AT.store(now + SWEEP_EVERY_SECS, Ordering::Relaxed);
                    state.last_sweep_at = Some(now);
                    sweep_in_background(now);
                }
            }));
            if res.is_err() {
                k2_core::log_debug!("[disk-watch] a tick panicked; the next runs in 60 s");
            }
        }
    });
}

#[cfg(test)]
#[path = "disk_watch_tests.rs"]
mod tests;
