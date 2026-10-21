//! Box usage and the disk guard (`prd-app-resources-disk-guard-v1`,
//! Slice A, 0.46.3): the view, the owner's settings, and the checkpoint
//! every App route calls before new disk work starts.
//!
//! ```text
//! GET  /cli/app/resources           [app, owner]   owner, Admin login, any agent passport
//! POST /cli/app/resources/rescan                   same; one at a time, rate-limited
//! POST /cli/app/resources/settings  {mode?, lines?} the owner only (owner token or Owner login)
//! ```
//!
//! - **Who sees what.** The owner and Admin logins see absolute folders
//!   and workspace ids; agents (and a K2 shell tab) see the agent's name,
//!   handle and address and the names of its biggest top-level folders,
//!   never a path (the box view's rule, `coordination.rs`). App passes,
//!   API keys and sessions get `wrong_credential`; Member logins
//!   `role_required`.
//! - **Numbers.** Disks are probed live (`k2_core::disk_guard`); the rows
//!   come from the scanner's latest snapshot (lane M). Before a scan has
//!   run, every App still has a row, with `diskBytes: null` (unknown is
//!   never 0).
//! - **The guard** ([`app_gate`], [`decide`]) runs where new disk work
//!   starts (build, install, deploy, data put, `k2 publish run`). It never
//!   touches running work, restarts or rollbacks. Mode `off` adds nothing.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use k2_core::app_grace::requests as reqs;
use k2_core::connect_users::Role;
use k2_core::deploy::store::{self, AppRow};
use k2_core::deploy::{self, codes};
use k2_core::disk_guard::{self as dg, Consumer, Finding, Level, Mode, Owner, Snapshot, Verdict};
use serde_json::{json, Value as J};

use super::routes::{actor, db_err, json_body, need_credential, no, s, Refusal, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/resources", "/cli/app/resources/rescan", "/cli/app/resources/settings"];
pub const POST_ROUTES: &[&str] = &["/cli/app/resources/rescan", "/cli/app/resources/settings"];

/// The refusal code for new disk work past the lines.
pub const DISK_LOW: &str = "disk_low";
/// A Member login on an Admin-floor read.
pub const ROLE_REQUIRED: &str = "role_required";

/// A walled App's first start copies its image (stage W).
pub const WALLED_FIRST_START: u64 = 256 * dg::MIB;

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/resources" => view(who, params),
        "/cli/app/resources/rescan" => rescan(who),
        "/cli/app/resources/settings" => settings(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

// ── callers ──────────────────────────────────────────────────────────

/// The owner token or an Owner/Admin login: sees folders and workspace ids.
pub(crate) fn is_person(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. })
}

/// The view's floor: any credential but App passes, and no Member login.
fn need_reader(who: &Who) -> R<()> {
    need_credential(who)?;
    if let Who::Login { role: Role::Member, .. } = who {
        return Err(no(ROLE_REQUIRED, 403, "box usage needs an admin login on this server"));
    }
    Ok(())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── the view ─────────────────────────────────────────────────────────

/// Paths whose filesystems the view and the watch track, labelled with
/// what they hold (`holds`): the scanner's one list (every K2 Postgres
/// cluster's real data directory once it is known).
pub fn tracked_paths() -> Vec<(String, PathBuf)> {
    super::disk_scan::tracked_paths()
}

/// An App's maintainer as a view row owner (`Owner::server()` for a parked
/// or removed workspace's App).
fn owner_of(conn: &rusqlite::Connection, ws: &str) -> Owner {
    let m = super::coordination::maintainer_with(conn, ws);
    if m.kind != "agent" {
        return Owner::server();
    }
    Owner { kind: "agent".into(), agent: m.agent, handle: m.handle, address: m.address, workspace_id: Some(ws.to_string()) }
}

/// Every App as a row: the scanner's row when it has one, else the App's
/// identity with unknown numbers.
fn app_rows(snap: Option<&Snapshot>) -> R<Vec<Consumer>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let apps = store::list_apps(&conn, None).map_err(db_err)?;
    let mut out = Vec::new();
    for a in &apps {
        if snap.is_some_and(|s| s.consumers.iter().any(|c| c.kind == "app" && c.id == a.app_id)) {
            continue;
        }
        let cfg = k2_core::app_grace::config::load(&conn, &a.app_id).unwrap_or_default();
        out.push(Consumer {
            kind: "app".into(),
            id: a.app_id.clone(),
            path: Some(
                if a.path == "open" {
                    "open".into()
                } else if a.isolation == "container" {
                    "walled".into()
                } else {
                    "process".into()
                },
            ),
            state: Some(a.state.clone()),
            owner: owner_of(&conn, &a.owner_workspace),
            budget_bytes: cfg.resources.disk_bytes.and_then(|b| u64::try_from(b).ok()),
            ..Consumer::default()
        });
    }
    Ok(out)
}

/// The agents' memory leaf's current use (Linux, cgroup v2).
fn agents_mem() -> Option<u64> {
    let st = k2_core::agents_leaf::status_json();
    let leaf = st.get("leaf").and_then(J::as_str)?;
    std::fs::read_to_string(Path::new(leaf).join("memory.current")).ok()?.trim().parse().ok()
}

fn owner_matches(o: &Owner, want: &str) -> bool {
    let w = want.trim().to_ascii_lowercase();
    [&o.handle, &o.address, &o.agent].iter().any(|v| v.as_deref().is_some_and(|x| x.to_ascii_lowercase() == w))
        || (w == "server" && !o.is_agent())
}

/// The plain-words suggestion when a disk is past the warn line.
fn suggestion(worst: Option<(&dg::Fs, Level)>, by: &[dg::OwnerTotal]) -> J {
    let Some((fs, lvl)) = worst else { return J::Null };
    if lvl == Level::Ok {
        return J::Null;
    }
    let mut s = format!("Disk {} is {}% full.", fs.label(), fs.used_pct);
    if let Some(top) = by.first() {
        let name = top.owner.handle.clone().unwrap_or_else(|| "an agent".into());
        s.push_str(&format!(" The biggest owner is {name} ({})", dg::human_gb(top.disk_bytes)));
        if let Some(a) = &top.owner.address {
            s.push_str(&format!(": k2 msg {a} \"…\""));
        }
    }
    json!(s)
}

/// GET `/cli/app/resources [app, owner]`.
fn view(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_reader(who)?;
    let person = is_person(who);
    let guard = dg::guard();
    let snap = dg::latest_snapshot();
    let mut consumers: Vec<Consumer> = snap.as_ref().map(|s| s.consumers.clone()).unwrap_or_default();
    consumers.extend(app_rows(snap.as_deref())?);
    // A snapshot cached before 0.46.3 may still hold K2's audit sentinels
    // as agent folders (`disk_scan::workspace_rows` skips them now).
    consumers.retain(|c| !(c.kind == "workspace" && k2_core::db::AUDIT_SENTINEL_IDS.contains(&c.id.as_str())));
    let full = Snapshot {
        consumers: consumers.clone(),
        k2_will_clear: snap.as_ref().and_then(|s| s.k2_will_clear.clone()),
        scan: snap.as_ref().map(|s| s.scan.clone()).unwrap_or_default(),
    };
    let by_owner = full.by_owner();

    // Disks: every tracked fs once, with what it holds and what no row
    // accounts for.
    let groups = dg::group(&tracked_paths());
    let mut worst: Option<(&dg::Fs, Level)> = None;
    let mut disks = Vec::new();
    for (fs, holds) in &groups {
        let level = fs.level(&guard.lines);
        if worst.is_none_or(|(_, l)| level > l) {
            worst = Some((fs, level));
        }
        let on_disk: Vec<u64> = consumers.iter().filter(|c| c.disk.as_deref() == Some(fs.id.as_str())).filter_map(|c| c.disk_bytes).collect();
        let not_measured = (!on_disk.is_empty()).then(|| fs.used_bytes.saturating_sub(on_disk.iter().sum()));
        let mut d = serde_json::to_value(fs).map_err(|e| no("internal", 500, e.to_string()))?;
        d["level"] = json!(level);
        d["holds"] = json!(holds);
        d["notMeasuredBytes"] = json!(not_measured);
        disks.push(d);
    }

    // Rows: filtered, and stripped for agents.
    let app_filter = params.get("app").map(|a| a.trim().to_ascii_lowercase()).filter(|a| !a.is_empty());
    let owner_filter = params.get("owner").map(|o| o.trim().to_string()).filter(|o| !o.is_empty());
    let rows: Vec<J> = consumers
        .iter()
        .filter(|c| app_filter.as_ref().is_none_or(|a| c.kind == "app" && &c.id == a))
        .filter(|c| owner_filter.as_ref().is_none_or(|o| owner_matches(&c.owner, o)))
        .map(|c| if person { c.clone() } else { c.for_agent() })
        .map(|c| serde_json::to_value(c).unwrap_or(J::Null))
        .collect();
    if let Some(a) = &app_filter {
        if rows.is_empty() {
            return Err(no(codes::APP_NOT_FOUND, 404, format!("no App '{a}' on this box (k2 app list)")));
        }
    }
    let by_owner_json: Vec<J> = by_owner
        .iter()
        .map(|t| {
            let mut o = t.owner.clone();
            if !person {
                o.workspace_id = None;
            }
            json!({ "owner": o, "diskBytes": t.disk_bytes })
        })
        .collect();
    let (mem_total, mem_avail) = dg::box_mem();
    let mut scan = serde_json::to_value(&full.scan).unwrap_or(J::Null);
    scan["diskAt"] = json!(now());
    scan["running"] = json!(full.scan.running || dg::scan_running());
    Ok(json!({
        "ok": true,
        "box": {
            "host": super::coordination::this_host(),
            "cores": dg::cores(),
            "memBytes": mem_total,
            "checkedAt": now(),
        },
        "guard": guard.to_json(),
        "disks": disks,
        "mem": { "totalBytes": mem_total, "availableBytes": mem_avail, "agentsBytes": agents_mem() },
        "cpu": { "load5": dg::load5() },
        "consumers": rows,
        "byOwner": by_owner_json,
        "k2WillClear": full.k2_will_clear,
        "scan": scan,
        "recent": dg::recent(dg::RECENT),
        "suggestion": suggestion(worst, &by_owner),
    }))
}

/// An App's measured disk from the latest snapshot (the box view's
/// `disk.usedBytes`); `None` = not measured.
pub fn app_disk_bytes(app_id: &str) -> Option<u64> {
    let snap = dg::latest_snapshot()?;
    snap.consumers.iter().find(|c| c.kind == "app" && c.id == app_id).and_then(|c| c.disk_bytes)
}

// ── rescan and settings ──────────────────────────────────────────────

/// POST `/cli/app/resources/rescan`.
fn rescan(who: &Who) -> R<J> {
    need_reader(who)?;
    let r = dg::request_rescan(now());
    if r == dg::Rescan::Started {
        dg::log_event("rescan", json!({ "by": actor(who) }));
    }
    Ok(r.to_json())
}

/// The command the owner would run for a patch (what an agent is told).
fn settings_command(p: &dg::SettingsPatch) -> String {
    let mut c = "k2 app resources settings".to_string();
    if let Some(m) = p.mode {
        c.push_str(&format!(" --mode {}", m.as_str()));
    }
    let size = |b: u64| dg::human_gib(b).replace(" GiB", "G").replace(" MiB", "M");
    for (flag, v) in [("--warn", p.warn_pct), ("--refuse", p.refuse_pct), ("--critical", p.critical_pct)] {
        if let Some(v) = v {
            c.push_str(&format!(" {flag} {v}"));
        }
    }
    for (flag, v) in [("--warn-free", p.warn_free_bytes), ("--reserve", p.reserve_bytes), ("--critical-free", p.critical_free_bytes)] {
        if let Some(v) = v {
            c.push_str(&format!(" {flag} {}", size(v)));
        }
    }
    c
}

/// POST `/cli/app/resources/settings {mode?, warnPct?, …}`: the owner
/// only. An empty body answers the current settings.
fn settings(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let patch: dg::SettingsPatch =
        serde_json::from_value(v).map_err(|e| no(codes::BAD_REQUEST, 400, format!("unreadable settings: {e}")))?;
    if !who.is_owner() {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            format!(
                "only the server's owner changes the disk guard (the owner token or an Owner login). Ask them to run: {}",
                settings_command(&patch)
            ),
        ));
    }
    let base = dg::load_settings();
    let mut saved = base.clone();
    if !patch.is_empty() {
        saved = patch.apply(&base, &actor(who), now()).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
        dg::save_settings(&saved).map_err(|e| no("io_error", 500, e))?;
        dg::log_event(
            "settings",
            json!({ "by": actor(who), "mode": saved.mode, "lines": saved.lines.to_json(), "was": { "mode": base.mode, "lines": base.lines.to_json() } }),
        );
    }
    let g = dg::guard();
    let mut out = json!({ "ok": true, "changed": !patch.is_empty(), "guard": g.to_json(), "settings": saved });
    if g.source == dg::ModeSource::Env {
        out["note"] = json!(format!(
            "{} is set in the daemon's environment: it decides the mode ({}) until it is removed",
            dg::ENV_MODE,
            g.mode.as_str()
        ));
    }
    Ok(out)
}

// ── the guard ────────────────────────────────────────────────────────

thread_local! {
    /// Set while a route whose outer call already passed the guard runs a
    /// nested step (an install's first deploy).
    static GUARDED: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` with the guard already decided (the nested step isn't checked
/// twice).
pub(crate) fn with_guard_done<T>(f: impl FnOnce() -> T) -> T {
    let was = GUARDED.with(|c| c.replace(true));
    let out = f();
    GUARDED.with(|c| c.set(was));
    out
}

/// The filesystems an App action writes: `~/.k2`, plus the walled tree
/// when the App is (or will be) walled and the box has it.
pub(crate) fn app_paths(walled: bool) -> Vec<PathBuf> {
    let mut v = vec![k2_core::paths::k2_home()];
    if walled && Path::new(k2_root_proto::apps::APP_HOME).exists() {
        v.push(PathBuf::from(k2_root_proto::apps::APP_HOME));
    }
    v
}

/// A stored Seed's size.
pub(crate) fn seed_size(hash: Option<&str>) -> Option<u64> {
    let h = hash?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::get_seed(&conn, h).ok().flatten().and_then(|r| u64::try_from(r.size_bytes).ok())
}

/// C1: the last Seed of the App a folder builds (its manifest id, or the
/// App with that folder), else 0.
pub(crate) fn build_need(folder: &Path) -> u64 {
    let id = std::fs::read_to_string(folder.join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<J>(&t).ok())
        .and_then(|m| m.get("id").and_then(J::as_str).map(str::to_ascii_lowercase));
    let live: Option<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let by_id = id.as_deref().and_then(|i| store::get_app(&conn, i).ok().flatten()).and_then(|a| a.live_seed);
        by_id.or_else(|| {
            conn.query_row(
                "SELECT live_seed FROM apps WHERE folder_path = ?1 AND live_seed IS NOT NULL LIMIT 1",
                rusqlite::params![folder.to_string_lossy()],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
        })
    };
    seed_size(live.as_deref()).unwrap_or(0)
}

/// C3: last live Seed × (2 if walled), + 256 MiB if it has never started
/// walled.
pub(crate) fn deploy_need(app: &AppRow) -> u64 {
    let walled = app.isolation == "container";
    let last = seed_size(app.live_seed.as_deref()).unwrap_or(0);
    let mut need = if walled { last.saturating_mul(2) } else { last };
    if walled && app.live_seed.is_none() {
        need = need.saturating_add(WALLED_FIRST_START);
    }
    need
}

/// The size of a file or folder, stopping at `cap` (C5).
pub(crate) fn size_capped(p: &Path, cap: u64) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![p.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&cur) else { continue };
        if meta.is_file() {
            total = total.saturating_add(meta.len());
        } else if meta.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&cur) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        }
        if total >= cap {
            return cap;
        }
    }
    total
}

/// The disk part of an answer (`diskWarning`).
pub(crate) fn warning_json(f: &Finding) -> J {
    let mut hint = f.sentence("deploy");
    if f.would_refuse {
        hint.push_str(" The guard is in warn mode: with it on, this would have been refused.");
    }
    if let Some(top) = biggest(1).first() {
        hint.push_str(&format!(" The biggest owner is {}.", top.0));
    }
    json!({
        "level": f.level,
        "mount": f.fs.label(),
        "usedPct": f.fs.used_pct,
        "freeBytes": f.fs.free_bytes,
        "wouldRefuse": f.would_refuse,
        "hint": hint,
    })
}

/// The memory warning (Q10): available RAM under 10% of the box. Never a
/// refusal. Names the three biggest App memory users the scan knows.
pub(crate) fn memory_warning() -> Option<J> {
    let (total, avail) = dg::box_mem();
    if !dg::memory_low(total, avail) {
        return None;
    }
    let mut apps: Vec<(String, u64)> = dg::latest_snapshot()
        .map(|s| s.consumers.iter().filter(|c| c.kind == "app").filter_map(|c| Some((c.id.clone(), c.mem_bytes?))).collect())
        .unwrap_or_default();
    apps.sort_by(|a, b| b.1.cmp(&a.1));
    apps.truncate(3);
    let names: Vec<String> = apps.iter().map(|(a, b)| format!("{a} {}", dg::human_gb(*b))).collect();
    Some(json!({
        "availableBytes": avail,
        "totalBytes": total,
        "biggestApps": apps.iter().map(|(a, b)| json!({ "app": a, "memBytes": b })).collect::<Vec<_>>(),
        "hint": format!(
            "This box has {} of {} RAM available (under 10%).{} It still goes ahead: K2 warns about memory, it never refuses on it.",
            avail.map(dg::human_gb).unwrap_or_default(),
            total.map(dg::human_gb).unwrap_or_default(),
            if names.is_empty() { String::new() } else { format!(" The biggest Apps: {}.", names.join(", ")) }
        ),
    }))
}

/// The biggest owners in words (`agent-nsi 104 GB`) with their `k2 msg`
/// line, and the JSON for the refusal.
fn biggest(n: usize) -> Vec<(String, Option<String>, J)> {
    let Some(snap) = dg::latest_snapshot() else { return vec![] };
    snap.by_owner()
        .into_iter()
        .take(n)
        .map(|t| {
            let name = t.owner.handle.clone().or(t.owner.agent.clone()).unwrap_or_else(|| "an agent".into());
            let mut owner = t.owner.clone();
            owner.workspace_id = None;
            (
                format!("{name} {}", dg::human_gb(t.disk_bytes)),
                t.owner.address.clone(),
                json!({ "owner": owner, "bytes": t.disk_bytes }),
            )
        })
        .collect()
}

/// `409 disk_low`: the whole story in the hint (the CLI prints only the
/// code and hint), the numbers beside it.
pub(crate) fn refusal(f: &Finding, action: &str) -> Refusal {
    let mut hint = f.sentence(action);
    let top = biggest(3);
    if !top.is_empty() {
        let parts: Vec<String> = top
            .iter()
            .map(|(words, addr, _)| match addr {
                Some(a) => format!("{words} (k2 msg {a} \"…\")"),
                None => words.clone(),
            })
            .collect();
        hint.push_str(&format!(" The biggest users: {}.", parts.join(", ")));
    }
    let snap = dg::latest_snapshot();
    let clears = snap.as_ref().and_then(|s| s.k2_will_clear.clone()).filter(|w| w.seeds_bytes > 0);
    if let Some(w) = &clears {
        hint.push_str(&format!(" K2 clears {} of old Seeds at the next sweep.", dg::human_gb(w.seeds_bytes)));
    }
    hint.push_str(" See k2 app resources.");
    if f.level == Level::Critical {
        hint.push_str(" To go ahead anyway: --disk-override \"<reason>\" (at critical it waits for the owner in person).");
    } else {
        hint.push_str(" To go ahead anyway: --disk-override \"<reason>\" (the owner at once; anyone else's waits for the owner).");
    }
    let mut r = no(DISK_LOW, 409, hint);
    r.extra = Some(json!({
        "level": f.level,
        "disk": f.disk_json(),
        "biggest": top.iter().map(|t| t.2.clone()).collect::<Vec<_>>(),
        "k2WillClearBytes": clears.map(|w| w.seeds_bytes),
        "scannedAt": snap.as_ref().and_then(|s| s.owners_scanned_at()),
    }));
    r
}

/// The reasons a held override carries (the disk numbers at filing time).
pub(crate) fn hold_reasons(f: &Finding, action: &str, reason: &str) -> Vec<String> {
    vec![
        format!(
            "disk {} is {}% full, {} free; this {action} needs about {}; the box keeps {}",
            f.fs.label(),
            f.fs.used_pct,
            dg::human_gb(f.fs.free_bytes),
            dg::human_gb(f.need_bytes),
            dg::human_gib(f.lines.reserve_bytes)
        ),
        if f.level == Level::Critical {
            "the disk is critical: every override waits for the owner in person".to_string()
        } else {
            "a disk override from anyone but the owner waits for the owner".to_string()
        },
        format!("reason: {reason}"),
    ]
}

/// What a checkpoint decided.
#[derive(Debug)]
pub(crate) enum Decision {
    /// Go ahead; merge these fields into the answer (empty in mode `off`).
    Go(J),
    /// `409 disk_low`.
    Refuse(Box<Refusal>),
    /// An override that waits for the owner (the caller files it).
    Hold { finding: Box<Finding>, reason: String },
}

/// The checkpoint, independent of how a hold is filed. `override_raw` is
/// the caller's `diskOverride` reason; `memory` adds the memory warning
/// (install, deploy).
pub(crate) fn decide(
    who: &Who,
    action: &str,
    subject: &str,
    paths: &[PathBuf],
    need: u64,
    override_raw: Option<&str>,
    app: Option<&AppRow>,
    memory: bool,
) -> R<Decision> {
    if GUARDED.with(|c| c.get()) {
        return Ok(Decision::Go(json!({})));
    }
    let verdict = dg::check(paths, need);
    let mode_on = dg::guard().mode != Mode::Off || !matches!(verdict, Verdict::Pass);
    let mut extra = json!({});
    if memory && mode_on {
        if let Some(m) = memory_warning() {
            extra["memoryWarning"] = m;
        }
    }
    let by = actor(who);
    let log_fields = |f: &Finding| {
        json!({ "action": action, "app": subject, "by": by, "level": f.level, "mount": f.fs.label(),
                "usedPct": f.fs.used_pct, "freeBytes": f.fs.free_bytes, "needBytes": f.need_bytes })
    };
    match verdict {
        Verdict::Pass => Ok(Decision::Go(extra)),
        Verdict::Warn(f) => {
            dg::log_event("warned", log_fields(&f));
            extra["diskWarning"] = warning_json(&f);
            Ok(Decision::Go(extra))
        }
        Verdict::Refuse(f) => {
            let reason = match override_raw.map(str::trim).filter(|r| !r.is_empty()) {
                None => {
                    dg::log_event("refused", log_fields(&f));
                    return Ok(Decision::Refuse(Box::new(refusal(&f, action))));
                }
                Some(r) => dg::clean_reason(r).map_err(|e| no(codes::BAD_REQUEST, 400, e))?,
            };
            let mut fields = log_fields(&f);
            fields["reason"] = json!(reason);
            // The replay of a request the owner approved in person: the
            // owner already decided (Q9).
            if super::approvals::manage_approved_now() {
                fields["via"] = json!("approved request");
                dg::log_event("override", fields);
                extra["diskOverride"] = json!({ "applied": true, "via": "approved", "reason": reason, "disk": f.disk_json() });
                return Ok(Decision::Go(extra));
            }
            if who.is_owner() && f.level < Level::Critical {
                dg::log_event("override", fields.clone());
                if let Some(a) = app {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, &a.app_id, "disk_override", &by, None, &fields) {
                        k2_core::log_debug!("[apps] audit {} disk_override: {e}", a.app_id);
                    }
                }
                extra["diskOverride"] = json!({ "applied": true, "via": "owner", "reason": reason, "disk": f.disk_json() });
                return Ok(Decision::Go(extra));
            }
            dg::log_event("held", fields);
            Ok(Decision::Hold { finding: Box::new(f), reason })
        }
    }
}

/// What an App route does after the checkpoint.
pub(crate) enum Gate {
    /// Go ahead, then [`decorate`] the answer with these fields.
    Go(J),
    /// The held override's answer (`202`, exit 2).
    Held(J),
}

/// One App checkpoint (C1, C2, C3, C5): decide, and file an override that
/// waits for the owner as a held `manage` request replaying `route`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn app_gate(
    who: &Who,
    route: &'static str,
    action: &str,
    label: &str,
    workspace: &str,
    app: Option<&AppRow>,
    paths: &[PathBuf],
    need: u64,
    body: &J,
    memory: bool,
) -> R<Gate> {
    match decide(who, action, label, paths, need, s(body, "diskOverride"), app, memory)? {
        Decision::Go(extra) => Ok(Gate::Go(extra)),
        Decision::Refuse(r) => Err(*r),
        Decision::Hold { finding, reason } => {
            let mut b = body.clone();
            // A shell tab's request replays as the owner, who must name the
            // workspace the tab acted for.
            if let Who::Shell { project_id, .. } = who {
                if b.get("workspace").is_none() {
                    b["workspace"] = json!(project_id);
                }
            }
            let r = super::approvals::file(&super::approvals::Filing {
                who,
                kind: reqs::KIND_MANAGE,
                app,
                label,
                workspace,
                body: json!({ "route": route, "body": b }),
                diff: json!({ "lines": [format!("k2 app {action} {label} --disk-override \"{reason}\"")], "route": route }),
                summary: format!("{action} {label} past the disk guard"),
                reasons: hold_reasons(&finding, action, &reason),
                reason: Some(&reason),
            })?;
            let mut out = super::approvals::held_answer(&r);
            out["disk"] = finding.disk_json();
            out["hint"] = json!(format!(
                "held for the owner: {} They approve it in person (k2 app approve {} from a terminal outside K2, or Settings → App Approvals).",
                finding.sentence(action),
                r.id
            ));
            Ok(Gate::Held(out))
        }
    }
}

/// Merge a checkpoint's fields into a route's answer.
pub(crate) fn decorate(out: &mut J, extra: J) {
    if let (Some(o), J::Object(m)) = (out.as_object_mut(), extra) {
        for (k, v) in m {
            o.insert(k, v);
        }
    }
}

/// The parts of a workspace row an agent may see, by top-level folder
/// name (exposed for the scanner: never a path).
pub fn top_parts(parts: &BTreeMap<String, u64>, n: usize) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = parts.iter().map(|(k, b)| (k.clone(), *b)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(n);
    v
}
