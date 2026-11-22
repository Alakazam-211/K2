//! The requesting server's compute store: retention, `k2 compute prune`,
//! `k2 compute pin` / `unpin`, `k2 compute retention` (0.46.17).
//!
//! The rule is [`k2_core::compute::retention`]: a job's fetched artifacts
//! and its blobs go once it ended more than the max age ago (default
//! 7 days), then the oldest until the store is under the cap (default
//! 20 GB or 10% of the disk); pinned jobs, jobs that haven't ended and
//! anything in use stay. This file deletes, on the daemon's ticker
//! ([`sweep_tick`], hourly, headless too) and on `prune`.
//!
//! **In use.** A fetch registers the attempt's folder for as long as it
//! pulls or reads a slice ([`use_artifacts`], [`InUse`]), and the folder
//! counts as in use for [`IN_USE_GRACE`] after (the CLI reads a big file
//! as many 4 MB requests). A removal checks and renames under the same
//! lock a fetch registers under, so a fetch either holds the folder
//! first (and it stays) or starts after it moved away (and pulls the
//! file from the node again). What goes is renamed into
//! `~/.k2/compute/.prune-trash` first, then deleted.
//!
//! **Only copies.** A secret job's files that the node deleted after this
//! server fetched them exist only here: the cap never takes them, only
//! the max age does. `retention` lists each with the date it goes, and a
//! store that they alone push over the cap says so instead of deleting.
//!
//! **Who.** `prune`, `pin`, `unpin` and `retention set` are the owner's
//! and Admins' (POST only); `retention` reads for the same people.
//! `k2 compute status` shows a one-line summary to every caller.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::brand::CLI_NAME;
use k2_core::compute::retention::{self as rt, human_bytes, human_secs, Keep, Why};
use k2_core::compute::{self as compute, codes, store};
use k2_core::log_debug;

use crate::compute_routes::{audit, b, job_for, json_body, need_admin, no, s, Who, R};

pub const PRUNE: &str = "/cli/compute/prune";
pub const RETENTION: &str = "/cli/compute/retention";
pub const RETENTION_SET: &str = "/cli/compute/retention/set";
pub const PIN: &str = "/cli/compute/pin";
pub const UNPIN: &str = "/cli/compute/unpin";

pub const ROUTES: &[&str] = &[PIN, PRUNE, RETENTION, RETENTION_SET, UNPIN];
/// POST only (405 on GET, the dispatcher's guard).
pub const POST_ROUTES: &[&str] = &[PIN, PRUNE, RETENTION_SET, UNPIN];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub(crate) fn handle(p: &str, who: &Who, body: &[u8]) -> R<serde_json::Value> {
    match p {
        PRUNE => prune_route(who, body),
        RETENTION => retention_route(who),
        RETENTION_SET => retention_set(who, body),
        PIN => pin_route(who, body, true),
        UNPIN => pin_route(who, body, false),
        _ => Err(no("not_found", 404, "no such compute route")),
    }
}

// ── in use ───────────────────────────────────────────────────────────

/// A folder a fetch used counts as in use this long after.
pub const IN_USE_GRACE: Duration = Duration::from_secs(600);

struct Use {
    active: usize,
    touched: Instant,
}

static IN_USE: Mutex<Option<HashMap<PathBuf, Use>>> = Mutex::new(None);

fn with_use<T>(f: impl FnOnce(&mut HashMap<PathBuf, Use>) -> T) -> T {
    let mut g = IN_USE.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// Holds a path in use until dropped.
pub struct InUse(PathBuf);

impl Drop for InUse {
    fn drop(&mut self) {
        with_use(|m| {
            if let Some(u) = m.get_mut(&self.0) {
                u.active = u.active.saturating_sub(1);
                u.touched = Instant::now();
            }
            // Old entries go (the map would grow by a folder per fetch).
            if m.len() > 4096 {
                m.retain(|_, u| u.active > 0 || u.touched.elapsed() < IN_USE_GRACE);
            }
        });
    }
}

/// Mark `p` in use (re-entrant: each holder counts).
pub fn use_path(p: &Path) -> InUse {
    with_use(|m| {
        let u = m.entry(p.to_path_buf()).or_insert(Use { active: 0, touched: Instant::now() });
        u.active += 1;
        u.touched = Instant::now();
    });
    InUse(p.to_path_buf())
}

/// `~/.k2/compute/artifacts/<job>-g<gen>`.
pub fn artifacts_dir(job_id: &str, generation: i64) -> PathBuf {
    compute::compute_dir().join("artifacts").join(format!("{job_id}-g{generation}"))
}

/// Mark one attempt's fetched files in use.
pub fn use_artifacts(job_id: &str, generation: i64) -> InUse {
    use_path(&artifacts_dir(job_id, generation))
}

fn busy(m: &HashMap<PathBuf, Use>, p: &Path, grace: Duration) -> bool {
    m.get(p).is_some_and(|u| u.active > 0 || u.touched.elapsed() < grace)
}

// ── the sweep ────────────────────────────────────────────────────────

/// One prune's options.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Remove ended jobs older than this (default: the policy's max age).
    pub older_than: Option<i64>,
    pub dry_run: bool,
    /// How long after a fetch its folder still counts as in use.
    pub grace: Duration,
    pub now: i64,
}

impl Opts {
    pub fn now(older_than: Option<i64>, dry_run: bool) -> Self {
        Self { older_than, dry_run, grace: IN_USE_GRACE, now: compute::now() }
    }
}

/// What a prune did (or, dry, would do).
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub dry_run: bool,
    pub policy: rt::Policy,
    pub disk_total: u64,
    pub max_age: i64,
    pub cap: u64,
    pub bytes_before: u64,
    /// Removed (or would be).
    pub removed: Vec<(rt::Unit, Why)>,
    /// Kept, and why.
    pub kept: Vec<(rt::Unit, Keep)>,
    /// The plan said remove, but at the moment of removal it was in use,
    /// running again, or pinned: left alone.
    pub skipped: Vec<(rt::Unit, &'static str)>,
}

impl Outcome {
    pub fn removed_bytes(&self) -> u64 {
        self.removed.iter().map(|(u, _)| u.bytes()).sum()
    }
    pub fn bytes_after(&self) -> u64 {
        self.bytes_before.saturating_sub(self.removed_bytes())
    }
}

static SWEEPING: AtomicBool = AtomicBool::new(false);
static LAST: Mutex<Option<(i64, u64, usize)>> = Mutex::new(None);

/// Delete what an earlier prune moved aside and didn't finish deleting.
fn clear_trash(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir.join(rt::TRASH_DIR));
}

/// Run the rule once (see the module docs). One prune at a time: a
/// second waits for the first.
pub fn prune(opts: &Opts) -> Result<Outcome, String> {
    static ONE: Mutex<()> = Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(|p| p.into_inner());
    let dir = compute::compute_dir();
    if !opts.dry_run {
        clear_trash(&dir);
    }
    let entries = rt::scan(&dir);
    let ids: BTreeSet<String> = entries.iter().filter_map(|e| e.job.clone()).collect();
    let (facts, pins, policy) = k2_core::db::read(|c| (rt::job_facts(c, &ids), rt::pins(c), rt::load(c)));
    // A pin that can't be read must still hold: remove nothing.
    let pins: BTreeSet<String> = pins.map_err(|e| format!("can't read the pins, so nothing was removed: {e}"))?.into_keys().collect();
    let disk_total = compute::advertise::probe(&dir).disk_total_bytes;
    let max_age = opts.older_than.unwrap_or_else(|| policy.max_age());
    let cap = policy.cap(disk_total);
    let in_use = |e: &rt::Entry| with_use(|m| busy(m, &e.path, opts.grace));
    let plan = rt::plan(rt::Input { entries, facts: &facts, pins: &pins, in_use: &in_use, now: opts.now, max_age, cap });
    let mut out = Outcome { dry_run: opts.dry_run, policy, disk_total, max_age, cap, bytes_before: plan.bytes_before, kept: plan.keep, ..Outcome::default() };
    if opts.dry_run {
        out.removed = plan.remove;
        return Ok(out);
    }
    let trash = dir.join(rt::TRASH_DIR);
    let mut n = 0usize;
    for (unit, why) in plan.remove {
        if let Some(reason) = recheck(&unit, why) {
            out.skipped.push((unit, reason));
            continue;
        }
        // Check and move aside under the lock a fetch registers under.
        let moved = with_use(|m| {
            if unit.entries.iter().any(|e| busy(m, &e.path, opts.grace)) {
                return Err("in use");
            }
            for e in &unit.entries {
                n += 1;
                if std::fs::create_dir_all(&trash).is_err() || std::fs::rename(&e.path, trash.join(format!("{n}"))).is_err() {
                    // Another filesystem, or gone already: delete in place.
                    let _ = if e.path.is_dir() { std::fs::remove_dir_all(&e.path) } else { std::fs::remove_file(&e.path) };
                }
            }
            Ok(())
        });
        match moved {
            Ok(()) => out.removed.push((unit, why)),
            Err(r) => out.skipped.push((unit, r)),
        }
    }
    clear_trash(&dir);
    *LAST.lock().unwrap_or_else(|p| p.into_inner()) = Some((opts.now, out.removed_bytes(), out.removed.len()));
    Ok(out)
}

/// The job again, just before its files go: a retry may have reopened
/// it, a person may have pinned it, a job row may have appeared.
fn recheck(unit: &rt::Unit, why: Why) -> Option<&'static str> {
    let job = unit.job.as_ref()?;
    if why == Why::Leftover {
        return None;
    }
    let ids: BTreeSet<String> = [job.clone()].into();
    let (facts, pinned) = k2_core::db::read(|c| {
        let f = rt::job_facts(c, &ids);
        let pins = rt::pins(c).ok();
        (f, pins)
    });
    let Some(pins) = pinned else { return Some("pins unreadable") };
    match facts.get(job) {
        None if why == Why::Orphan => None,
        None => Some("job changed"),
        Some(_) if why == Why::Orphan => Some("job appeared"),
        Some(f) if !f.ended => Some("running again"),
        Some(f) if pins.contains_key(job) || f.parent.as_ref().is_some_and(|p| pins.contains_key(p)) => Some("pinned"),
        Some(_) => None,
    }
}

/// The ticker's hourly call: one sweep on its own thread (a big store
/// takes a while to measure), never two at once.
pub fn sweep_tick() {
    if SWEEPING.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new().name("k2-compute-retention".into()).spawn(|| {
        match prune(&Opts::now(None, false)) {
            Ok(o) if !o.removed.is_empty() => {
                log_debug!("[compute] retention: removed {} job(s)' files, {} ({} kept, {} skipped)", o.removed.len(), human_bytes(o.removed_bytes()), o.kept.len(), o.skipped.len())
            }
            Ok(_) => {}
            Err(e) => log_debug!("[compute] retention sweep: {e}"),
        }
        SWEEPING.store(false, Ordering::SeqCst);
    });
    if spawned.is_err() {
        SWEEPING.store(false, Ordering::SeqCst);
    }
}

// ── what status shows ────────────────────────────────────────────────

/// The store's size and policy, for `k2 compute status` and `retention`.
pub fn summary() -> serde_json::Value {
    let dir = compute::compute_dir();
    let entries = rt::scan(&dir);
    let ids: BTreeSet<String> = entries.iter().filter_map(|e| e.job.clone()).collect();
    let (policy, pins, facts) = k2_core::db::read(|c| (rt::load(c), rt::pins(c).unwrap_or_default(), rt::job_facts(c, &ids)));
    let disk_total = compute::advertise::probe(&dir).disk_total_bytes;
    let (mut art, mut art_dirs, mut blob, mut blob_files, mut pinned_bytes) = (0u64, 0usize, 0u64, 0usize, 0u64);
    let mut jobs = BTreeSet::new();
    // Only copies (ended, not pinned): bytes and when the age rule takes each.
    let mut only: std::collections::BTreeMap<String, (u64, i64)> = std::collections::BTreeMap::new();
    for e in &entries {
        if e.is_blob() {
            blob += e.bytes;
            blob_files += 1;
        } else {
            art += e.bytes;
            art_dirs += 1;
        }
        if let Some(j) = &e.job {
            jobs.insert(j.clone());
            if pins.contains_key(j) {
                pinned_bytes += e.bytes;
            } else if let Some(f) = facts.get(j).filter(|f| f.only_copy && f.ended) {
                let until = f.ended_at.unwrap_or(e.mtime) + policy.max_age();
                only.entry(j.clone()).or_insert((0, until)).0 += e.bytes;
            }
        }
    }
    let only_bytes: u64 = only.values().map(|(b, _)| *b).sum();
    let cap = policy.cap(disk_total);
    let total = art + blob;
    let last = *LAST.lock().unwrap_or_else(|p| p.into_inner());
    let mut line = format!(
        "compute store: {} (artifacts {} in {art_dirs} folders, blobs {} in {blob_files} files) · {}",
        human_bytes(total),
        human_bytes(art),
        human_bytes(blob),
        policy.line(disk_total)
    );
    if !pins.is_empty() {
        line.push_str(&format!(" · {} pinned job(s)", pins.len()));
    }
    if only_bytes > cap {
        line.push_str(&format!(" · over the cap: {} held as only copies (they go by age, never for the cap)", human_bytes(only_bytes)));
    } else if total > cap {
        line.push_str(&format!(" · OVER THE CAP ({cli} compute prune --dry-run shows what goes)", cli = CLI_NAME));
    }
    serde_json::json!({
        "bytes": total,
        "artifactBytes": art,
        "artifactFolders": art_dirs,
        "blobBytes": blob,
        "blobFiles": blob_files,
        "jobs": jobs.len(),
        "pinnedJobs": pins.len(),
        "pinnedBytes": pinned_bytes,
        "onlyCopyBytes": only_bytes,
        "onlyCopies": only.iter().map(|(j, (b, until))| serde_json::json!({"job": j, "short": compute::message::short(j), "bytes": b, "until": until})).collect::<Vec<_>>(),
        "policy": policy_json(&policy, disk_total),
        "lastSweep": last.map(|(at, bytes, units)| serde_json::json!({"at": at, "removedBytes": bytes, "removedUnits": units})),
        "path": dir.display().to_string(),
        "line": line,
    })
}

fn policy_json(p: &rt::Policy, disk_total: u64) -> serde_json::Value {
    serde_json::json!({
        "maxAgeSecs": p.max_age(),
        "maxAgeFrom": if p.max_age_secs.is_some() { "set" } else { "default" },
        "capBytes": p.cap(disk_total),
        "capFrom": p.cap_from(),
        "diskTotalBytes": disk_total,
        "line": p.line(disk_total),
    })
}

// ── routes ───────────────────────────────────────────────────────────

fn unit_json(u: &rt::Unit, reason: &str, now: i64) -> serde_json::Value {
    serde_json::json!({
        "job": u.job,
        "short": u.job.as_deref().map(compute::message::short),
        "bytes": u.bytes(),
        "files": u.files(),
        "artifactFolders": u.entries.iter().filter(|e| e.kind == rt::Kind::Artifacts).count(),
        "blobFiles": u.entries.iter().filter(|e| e.is_blob()).count(),
        "ageSecs": (now - u.at).max(0),
        "reason": reason,
        "paths": u.entries.iter().map(|e| e.path.display().to_string()).collect::<Vec<_>>(),
    })
}

/// The JSON and the lines for people (`text`) of one prune.
pub fn outcome_json(o: &Outcome, now: i64) -> serde_json::Value {
    let count = |w: Why| o.removed.iter().filter(|(_, x)| *x == w).count();
    let (mut art_b, mut art_n, mut blob_b, mut blob_n) = (0u64, 0usize, 0u64, 0usize);
    for (u, _) in &o.removed {
        for e in &u.entries {
            if e.is_blob() {
                blob_b += e.bytes;
                blob_n += 1;
            } else {
                art_b += e.bytes;
                art_n += 1;
            }
        }
    }
    let kept_n = |k: Keep| o.kept.iter().filter(|(_, x)| *x == k).count();
    let kept_b = |k: Keep| o.kept.iter().filter(|(_, x)| *x == k).map(|(u, _)| u.bytes()).sum::<u64>();
    let stuck: u64 = [Keep::Pinned, Keep::Active, Keep::InUse].into_iter().map(kept_b).sum();
    let only_b = kept_b(Keep::OnlyCopy);
    let verb = if o.dry_run { "would remove" } else { "removed" };
    let mut text = vec![format!(
        "{verb} {} job(s)' files: {} (artifacts {} in {art_n} folders, blobs {} in {blob_n} files)",
        o.removed.len(),
        human_bytes(o.removed_bytes()),
        human_bytes(art_b),
        human_bytes(blob_b)
    )];
    let mut why = Vec::new();
    for (w, label) in [(Why::Age, format!("ended more than {} ago", human_secs(o.max_age))), (Why::Cap, format!("oldest, to get under the {} cap", human_bytes(o.cap))), (Why::Orphan, "no such job on this server".into()), (Why::Leftover, "a transfer's leftover bundle".into())] {
        if count(w) > 0 {
            why.push(format!("{} {label}", count(w)));
        }
    }
    if !why.is_empty() {
        text.push(format!("  why: {}", why.join("; ")));
    }
    text.push(format!(
        "kept: {} pinned ({}), {} not ended, {} in use (a fetch read it in the last {}), {} only copies ({}), {} inside the rule ({})",
        kept_n(Keep::Pinned),
        human_bytes(kept_b(Keep::Pinned)),
        kept_n(Keep::Active),
        kept_n(Keep::InUse),
        human_secs(IN_USE_GRACE.as_secs() as i64),
        kept_n(Keep::OnlyCopy),
        human_bytes(only_b),
        kept_n(Keep::Young),
        human_bytes(kept_b(Keep::Young))
    ));
    if !o.skipped.is_empty() {
        text.push(format!("skipped at the last moment: {} ({})", o.skipped.len(), o.skipped.iter().map(|(_, r)| *r).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", ")));
    }
    text.push(format!("store: {} before, {} after · {}", human_bytes(o.bytes_before), human_bytes(o.bytes_after()), o.policy.line(o.disk_total)));
    let over = o.bytes_after() > o.cap;
    if only_b > o.cap {
        text.push(format!(
            "over the cap: {} held as only copies (secret jobs' files the node deleted after this server fetched them): never removed for the cap, each goes {} after its job ended",
            human_bytes(only_b),
            human_secs(o.max_age)
        ));
    } else if over {
        text.push(format!(
            "still over the cap: pinned, unfinished and in-use files hold {} ({cli} compute unpin <job>, or {cli} compute retention set --cap SIZE)",
            human_bytes(stuck),
            cli = CLI_NAME
        ));
    }
    for (u, w) in o.removed.iter().take(20) {
        text.push(format!("  {:<9} {:>9}  {}  {} old", u.job.as_deref().map(compute::message::short).unwrap_or("(file)"), human_bytes(u.bytes()), w.word(), human_secs((now - u.at).max(0))));
    }
    if o.removed.len() > 20 {
        text.push(format!("  … and {} more (--json lists them all)", o.removed.len() - 20));
    }
    if o.dry_run {
        text.push("nothing was deleted (--dry-run); run it again without --dry-run to remove these".into());
    }
    serde_json::json!({
        "dryRun": o.dry_run,
        "maxAgeSecs": o.max_age,
        "capBytes": o.cap,
        "policy": policy_json(&o.policy, o.disk_total),
        "bytesBefore": o.bytes_before,
        "bytesAfter": o.bytes_after(),
        "overCap": over,
        "removed": {
            "units": o.removed.len(),
            "bytes": o.removed_bytes(),
            "artifactFolders": art_n,
            "artifactBytes": art_b,
            "blobFiles": blob_n,
            "blobBytes": blob_b,
            "byReason": {"age": count(Why::Age), "cap": count(Why::Cap), "orphan": count(Why::Orphan), "leftover": count(Why::Leftover)},
        },
        "kept": {
            "pinned": kept_n(Keep::Pinned), "pinnedBytes": kept_b(Keep::Pinned),
            "active": kept_n(Keep::Active), "activeBytes": kept_b(Keep::Active),
            "inUse": kept_n(Keep::InUse), "inUseBytes": kept_b(Keep::InUse),
            "young": kept_n(Keep::Young), "youngBytes": kept_b(Keep::Young),
            "onlyCopy": kept_n(Keep::OnlyCopy), "onlyCopyBytes": only_b,
        },
        "skipped": o.skipped.iter().map(|(u, r)| unit_json(u, r, now)).collect::<Vec<_>>(),
        "items": o.removed.iter().map(|(u, w)| unit_json(u, w.word(), now)).collect::<Vec<_>>(),
        "text": text,
    })
}

/// `POST /cli/compute/prune {olderThan?, dryRun?}`.
fn prune_route(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_admin(who)?;
    let v = json_body(body)?;
    let older_than = s(&v, "olderThan").map(rt::parse_duration).transpose().map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    let dry_run = b(&v, "dryRun") == Some(true);
    let opts = Opts::now(older_than, dry_run);
    let o = prune(&opts).map_err(|e| no("internal", 500, e))?;
    let out = outcome_json(&o, opts.now);
    if !dry_run {
        audit(who, "compute_prune", None, None, None, serde_json::json!({"olderThanSecs": older_than, "removed": out["removed"].clone()}));
    }
    Ok(out)
}

/// `GET /cli/compute/retention`: usage, policy, pins.
fn retention_route(who: &Who) -> R<serde_json::Value> {
    need_admin(who)?;
    let mut v = summary();
    let pins = k2_core::db::read(rt::pins).map_err(|e| no("internal", 500, e))?;
    let mut text = vec![v["line"].as_str().unwrap_or_default().to_string()];
    text.push(format!("  where: {}", v["path"].as_str().unwrap_or_default()));
    if let Some(l) = v["lastSweep"].as_object() {
        text.push(format!(
            "  last sweep: {} ago, removed {}",
            human_secs((compute::now() - l["at"].as_i64().unwrap_or(0)).max(0)),
            human_bytes(l["removedBytes"].as_u64().unwrap_or(0))
        ));
    }
    for p in pins.values().take(50) {
        text.push(format!("  pinned {} by {}", compute::message::short(&p.job), p.by));
    }
    for c in v["onlyCopies"].as_array().cloned().unwrap_or_default().iter().take(50) {
        text.push(format!(
            "  {} {}: only copy, kept until {}",
            c["short"].as_str().unwrap_or_default(),
            human_bytes(c["bytes"].as_u64().unwrap_or(0)),
            utc_date(c["until"].as_i64().unwrap_or(0))
        ));
    }
    text.push(format!(
        "change it: {cli} compute retention set --max-age 7d --cap 20G (or default / auto); remove now: {cli} compute prune",
        cli = CLI_NAME
    ));
    v["pins"] = serde_json::to_value(pins.values().collect::<Vec<_>>()).unwrap_or_default();
    v["text"] = text.into();
    Ok(v)
}

/// `2026-11-29 14:00 UTC`.
fn utc_date(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_else(|| secs.to_string())
}

/// `POST /cli/compute/retention/set {maxAge?: "7d"|"default", cap?: "20G"|"auto"}`.
fn retention_set(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_admin(who)?;
    let v = json_body(body)?;
    let bad = |e: String| no(codes::BAD_REQUEST, 400, e);
    if s(&v, "maxAge").is_none() && s(&v, "cap").is_none() {
        return Err(bad("name --max-age DURATION (or default) and/or --cap SIZE (or auto)".into()));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut p = rt::load(&conn);
    match s(&v, "maxAge") {
        Some("default") => p.max_age_secs = None,
        Some(d) => p.max_age_secs = Some(rt::parse_duration(d).map_err(bad)?),
        None => {}
    }
    match s(&v, "cap") {
        Some("auto") | Some("default") => p.cap_bytes = None,
        Some(c) => p.cap_bytes = Some(rt::parse_size(c).map_err(bad)?),
        None => {}
    }
    rt::save(&conn, &p, &who.label(), compute::now()).map_err(bad)?;
    store::record_event(&conn, &who.label(), "compute_retention_set", None, None, None, serde_json::to_value(&p).unwrap_or_default());
    drop(conn);
    let disk_total = compute::advertise::probe(&compute::compute_dir()).disk_total_bytes;
    let line = p.line(disk_total);
    Ok(serde_json::json!({"policy": policy_json(&p, disk_total), "text": [format!("compute store retention: {line} (the hourly sweep applies it; {cli} compute prune --dry-run shows what goes now)", cli = CLI_NAME)]}))
}

/// `POST /cli/compute/pin|unpin {job}`.
fn pin_route(who: &Who, body: &[u8], on: bool) -> R<serde_json::Value> {
    need_admin(who)?;
    let v = json_body(body)?;
    let j = job_for(who, s(&v, "job"))?;
    let short = compute::message::short(&j.id).to_string();
    let changed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let c = rt::set_pin(&conn, &j.id, on, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
        if c {
            store::record_event(&conn, &who.label(), if on { "compute_pin" } else { "compute_unpin" }, Some(&j.node_id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({}));
        }
        c
    };
    // Which of its files are on this server (a split run: its shards').
    let mut jobs = vec![j.clone()];
    if !j.ctl.shards.is_empty() {
        jobs.extend(crate::compute_shards::children(&j).into_iter().map(|(k, _)| k));
    }
    let (mut here, mut all, mut bytes) = (0usize, 0usize, 0u64);
    for k in &jobs {
        for a in compute::result::outputs(k).map(|o| o.artifacts).unwrap_or_default() {
            all += 1;
            if crate::compute_ws::artifact_cache_path(&k.id, k.generation, &a.sha256).is_file() {
                here += 1;
                bytes += a.bytes;
            }
        }
    }
    let text = if on {
        let mut t = vec![format!(
            "{} {short}: retention never removes its files on this server ({here} of its {all} artifacts are here, {}) until {cli} compute unpin {short}",
            if changed { "pinned" } else { "already pinned" },
            human_bytes(bytes),
            cli = CLI_NAME
        )];
        if here < all {
            t.push(format!(
                "the other {} are only on the node, which drops them 7 days after the job ended: {cli} compute fetch {short} copies them here, and the pin keeps them",
                all - here,
                cli = CLI_NAME
            ));
        }
        t
    } else {
        vec![format!(
            "{} {short}: its files here follow the retention rule again ({cli} compute retention shows it)",
            if changed { "unpinned" } else { "wasn't pinned:" },
            cli = CLI_NAME
        )]
    };
    Ok(serde_json::json!({"job": j.id, "short": short, "pinned": on, "changed": changed, "filesHere": here, "files": all, "bytesHere": bytes, "text": text}))
}

/// `k2 compute retry` of a `--dirty` job whose blob retention removed: a
/// clear refusal instead of a transfer that fails on the node.
pub(crate) fn source_kept(j: &store::JobRow) -> R<()> {
    let dirty = j.plan().and_then(|p| p.src).is_some_and(|s| s.dirty.is_some());
    if !dirty || crate::compute_ws::dirty_blob_path(&j.id).is_file() {
        return Ok(());
    }
    Err(no(
        "source_pruned",
        409,
        "its uncommitted changes (--dirty) are no longer on this server (the compute store's retention removed them after the job ended); submit it again",
    ))
}

#[cfg(test)]
#[path = "compute_retention_tests.rs"]
mod tests;
