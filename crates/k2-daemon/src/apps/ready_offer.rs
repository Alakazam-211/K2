//! "Get this server ready for Apps" (0.46.16, Rosson: K2 Apps should be
//! ready on every server). Servers we host are prepared by the stock
//! provisioner; a server someone installed or upgraded themselves gets a
//! one-click owner card instead. The daemon owns all of it: when to offer
//! the card, who sees it, the run and the restart it may need. The app only
//! shows what this module says.
//!
//! **When.** The card is offered when the box is Linux, Apps are on, the
//! doctor says NOT READY, no prepare is running, and a trigger is waiting:
//! - `upgrade`: the first start of a release with this card (every release
//!   since 0.46.3 runs Apps, so this is "the box upgraded to a release that
//!   supports Apps"; a fresh install counts too);
//! - `subscription:<tier>`: a paid plan (`single`, `pro`) starts on this
//!   server's K2 Connect subdomain (a tier change from a known free or empty
//!   tier; the first reading after an update only records the tier, which
//!   the `upgrade` trigger already covers);
//! - `apps_page`: the owner opens Settings → Cores → Apps.
//!
//! Each trigger is offered once: answered ("Not now", closing the card, or
//! running it) it never comes back. The owner can still run the same thing
//! any time from Settings → Cores → Apps → This server or with
//! `k2 app box prepare`.
//!
//! **Who.** The owner token and Owner logins only. Admins, Members and
//! agents read `{owner: false}` and can't answer, run or restart.
//!
//! **What it runs.** Never anything without the owner's click. The run is
//! the owner's `k2 app box prepare --install-podman` through the same root
//! helper path (`box_prep::prepare_core`), with `--cores-mem-min` sized by
//! the box's RAM (`k2_core::agents_leaf::cores_floor_mib`) unless the box
//! already has a cores floor, then `k2 app engine install` when the engine
//! isn't answering, then the doctor. It holds the shared root latch.
//!
//! **The restart.** The agents leaf (agent sessions' own memory budget)
//! needs the daemon's unit to start again with the cores drop-in. The run
//! never restarts: the card offers "Restart now" or "Tonight at 3 am" (the
//! owner's local time: the app's zone, else the owner's `ownerTimezone`,
//! else the box clock), because a restart signs guests out of older App
//! sites. The card says how many guests are connected and signed in right
//! now (`GET /cli/app/sessions/live`, counts only). A start after the
//! restart was scheduled means it happened: the schedule is cleared.
//!
//! **State** (offers, answers, the last run, the scheduled restart) lives in
//! `~/.k2/apps-ready-offer.json`, like the automatic prep's status file: no
//! migration, kept across restarts.

use std::path::PathBuf;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::box_prep::PrepareOpts;
use super::podman_install::PodmanOpts;
use super::routes::{need_credential, no, R};
use crate::compute_routes::Who;
use k2_core::brand::PRODUCT_NAME;
use k2_core::connect_users::Role;

pub const ROUTE: &str = "/cli/app/box/ready-offer";
pub const TRIGGER_ROUTE: &str = "/cli/app/box/ready-offer/trigger";
pub const DISMISS_ROUTE: &str = "/cli/app/box/ready-offer/dismiss";
pub const RUN_ROUTE: &str = "/cli/app/box/ready-offer/run";
pub const RESTART_ROUTE: &str = "/cli/app/box/ready-offer/restart";
/// Guests connected and signed in right now, per App (owner only).
pub const SESSIONS_LIVE_ROUTE: &str = "/cli/app/sessions/live";

pub const ROUTES: &[&str] = &[ROUTE, TRIGGER_ROUTE, DISMISS_ROUTE, RUN_ROUTE, RESTART_ROUTE, SESSIONS_LIVE_ROUTE];
pub const POST_ROUTES: &[&str] = &[TRIGGER_ROUTE, DISMISS_ROUTE, RUN_ROUTE, RESTART_ROUTE];

pub const TITLE: &str = "Get this server ready for Apps";
pub const LINE: &str = "Installs Podman and system packages, sets up the App user and firewall. No restart now: if one is needed, \
                        you pick when (a short restart signs guests out of older App sites).";
/// The hour "Tonight" means, in the owner's time.
pub const TONIGHT_HOUR: u32 = 3;
/// Who the audit names for the boot and tier triggers.
pub const ACTOR: &str = "k2:apps-ready";

pub const TRIGGER_UPGRADE: &str = "upgrade";
pub const TRIGGER_APPS_PAGE: &str = "apps_page";
pub const TRIGGER_SUBSCRIPTION: &str = "subscription";

// ── state ────────────────────────────────────────────────────────────

/// One trigger, offered once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub key: String,
    pub at: i64,
    #[serde(default)]
    pub answered_at: Option<i64>,
    /// `dismissed` or `ran`.
    #[serde(default)]
    pub answer: Option<String>,
}

/// The last run (the card's progress and result).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Run {
    /// `running`, `done` or `failed`.
    pub state: String,
    /// `prepare`, `engine`, `check`, `finished`.
    pub phase: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub by: String,
    pub cores_mem_min_mib: Option<u64>,
    pub changed: Vec<String>,
    pub error: Option<String>,
    pub ready: Option<bool>,
    /// The doctor rows that still block (`{id, status, detail, fix}`).
    pub blocking: Vec<J>,
    pub restart_needed: bool,
}

/// A restart the owner scheduled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Restart {
    pub at: i64,
    pub zone: String,
    pub scheduled_at: i64,
    pub by: String,
}

/// `~/.k2/apps-ready-offer.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Saved {
    pub triggers: Vec<Trigger>,
    /// The `upgrade` trigger was recorded (once per server).
    pub upgrade_noted: bool,
    /// The last plan tier heard (`free`, `single`, `pro`).
    pub last_tier: Option<String>,
    pub run: Option<Run>,
    pub restart: Option<Restart>,
}

/// One read-modify-write at a time (routes, the run thread, the boot).
static STORE_LOCK: Mutex<()> = Mutex::new(());

pub struct Store {
    pub path: PathBuf,
}

impl Store {
    pub fn live() -> Self {
        Store { path: k2_core::paths::k2_home().join("apps-ready-offer.json") }
    }

    /// Missing = nothing yet; unreadable = nothing yet (logged).
    pub fn load(&self) -> Saved {
        match std::fs::read(&self.path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                k2_core::log_debug!("[apps] {}: {e}; starting fresh", self.path.display());
                Saved::default()
            }),
            Err(_) => Saved::default(),
        }
    }

    fn save(&self, s: &Saved) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
        k2_core::fs_atomic::atomic_write(&self.path, &bytes).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    /// Load, change, save, under the store lock.
    pub fn update<T>(&self, f: impl FnOnce(&mut Saved) -> T) -> Result<T, String> {
        let _g = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut s = self.load();
        let out = f(&mut s);
        self.save(&s)?;
        Ok(out)
    }
}

// ── the rules (pure) ─────────────────────────────────────────────────

/// Record `key` once. `true` = new.
pub fn note_trigger(s: &mut Saved, key: &str, now: i64) -> bool {
    if s.triggers.iter().any(|t| t.key == key) {
        return false;
    }
    s.triggers.push(Trigger { key: key.to_string(), at: now, answered_at: None, answer: None });
    true
}

/// The oldest trigger nobody has answered.
pub fn pending(s: &Saved) -> Option<&Trigger> {
    s.triggers.iter().find(|t| t.answered_at.is_none())
}

/// Answer every waiting trigger (`ran`) or one (`dismissed`).
pub fn answer(s: &mut Saved, key: Option<&str>, how: &str, now: i64) -> usize {
    let mut n = 0;
    for t in s.triggers.iter_mut().filter(|t| t.answered_at.is_none() && key.is_none_or(|k| k == t.key)) {
        t.answered_at = Some(now);
        t.answer = Some(how.to_string());
        n += 1;
    }
    n
}

pub fn is_paid(tier: &str) -> bool {
    matches!(tier.trim().to_ascii_lowercase().as_str(), "single" | "pro")
}

/// A plan tier reading. A paid plan starting (from a known free or empty
/// tier) is a trigger; the first reading only records the tier.
pub fn on_tier(s: &mut Saved, tier: Option<&str>, now: i64) -> Option<String> {
    let new = tier.map(|t| t.trim().to_ascii_lowercase()).unwrap_or_default();
    let started = match s.last_tier.as_deref() {
        Some(prev) => !is_paid(prev) && is_paid(&new),
        None => false,
    };
    s.last_tier = Some(new.clone());
    let key = format!("{TRIGGER_SUBSCRIPTION}:{new}");
    (started && note_trigger(s, &key, now)).then_some(key)
}

/// Boot: the `upgrade` trigger once; a scheduled restart is done (this is a
/// new process); a run this process didn't finish failed.
pub fn on_boot(s: &mut Saved, now: i64) {
    if !s.upgrade_noted {
        note_trigger(s, TRIGGER_UPGRADE, now);
        s.upgrade_noted = true;
    }
    s.restart = None;
    if let Some(r) = s.run.as_mut().filter(|r| r.state == "running") {
        r.state = "failed".into();
        r.ended_at = Some(now);
        r.error = Some(format!("{PRODUCT_NAME} restarted while the setup ran; run it again (it picks up where it stopped)"));
    }
}

fn row<'a>(doctor: &'a J, id: &str) -> Option<&'a J> {
    doctor["checks"].as_array()?.iter().find(|c| c["id"] == id)
}

fn row_ok(doctor: &J, id: &str) -> bool {
    row(doctor, id).is_some_and(|c| c["status"] == "ok")
}

/// The agents leaf waits for a daemon restart: the cores drop-in is in
/// place but agent sessions still share the daemon's memory.
pub fn restart_needed(doctor: &J) -> bool {
    row_ok(doctor, "coresSlice") && row(doctor, "agentsLeaf").is_some_and(|c| c["status"] != "ok")
}

/// The doctor rows that block READY (the daemon's `blocking` list, else
/// every row that isn't ok or a warning).
pub fn blocking_rows(doctor: &J) -> Vec<J> {
    let checks = doctor["checks"].as_array().cloned().unwrap_or_default();
    let named: Option<Vec<&str>> = doctor["blocking"].as_array().map(|a| a.iter().filter_map(J::as_str).collect());
    checks
        .into_iter()
        .filter(|c| match &named {
            Some(ids) => c["id"].as_str().is_some_and(|id| ids.contains(&id)),
            None => !matches!(c["status"].as_str(), Some("ok" | "warn")),
        })
        .map(|c| json!({"id": c["id"], "status": c["status"], "detail": c["detail"], "fix": c["fix"]}))
        .collect()
}

/// What the box looks like for the offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub linux: bool,
    pub apps_on: bool,
    pub ready: bool,
}

/// The card to offer now, if any.
pub fn offer(f: &Facts, s: &Saved) -> Option<J> {
    if !f.linux || !f.apps_on || f.ready || s.run.as_ref().is_some_and(|r| r.state == "running") {
        return None;
    }
    let t = pending(s)?;
    Some(json!({ "trigger": t.key, "at": t.at, "title": TITLE, "line": LINE }))
}

/// The next `hour`:00 after `now` in `zone` (an IANA name), else the box
/// clock: `(unix seconds, the zone's name)`.
pub fn next_local_hour(now: i64, zone: Option<&str>, hour: u32) -> Result<(i64, String), String> {
    use chrono::TimeZone;
    fn next_in<Z: TimeZone>(tz: &Z, now: i64, hour: u32) -> Option<i64> {
        let local = chrono::Utc.timestamp_opt(now, 0).single()?.with_timezone(tz);
        let mut day = local.date_naive();
        for _ in 0..3 {
            if let Some(c) = tz.from_local_datetime(&day.and_hms_opt(hour, 0, 0)?).earliest() {
                if c.timestamp() > now {
                    return Some(c.timestamp());
                }
            }
            day = day.succ_opt()?;
        }
        None
    }
    match zone.map(str::trim).filter(|z| !z.is_empty()) {
        Some(z) => {
            let tz: chrono_tz::Tz = z.parse().map_err(|_| format!("\"{z}\" is not an IANA time zone (for example America/Denver)"))?;
            next_in(&tz, now, hour).map(|t| (t, tz.name().to_string())).ok_or_else(|| "no such time tonight".to_string())
        }
        None => next_in(&chrono::Local, now, hour)
            .map(|t| (t, k2_core::heartbeats::when::server_zone_name()))
            .ok_or_else(|| "no such time tonight".to_string()),
    }
}

fn is_owner(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. })
}

fn owner_only(who: &Who, what: &str) -> R<()> {
    need_credential(who)?;
    if is_owner(who) {
        Ok(())
    } else {
        Err(no("owner_only", 403, format!("{what} is the server owner's")))
    }
}

fn guests_json(connected: &[(String, u64)], signed_in: &[(String, u64)]) -> J {
    let mut apps: std::collections::BTreeMap<&str, (u64, u64)> = std::collections::BTreeMap::new();
    for (a, n) in connected {
        apps.entry(a).or_default().0 = *n;
    }
    for (a, n) in signed_in {
        apps.entry(a).or_default().1 = *n;
    }
    json!({
        "connected": connected.iter().map(|(_, n)| n).sum::<u64>(),
        "signedIn": signed_in.iter().map(|(_, n)| n).sum::<u64>(),
        "apps": apps.into_iter().map(|(a, (c, s))| json!({"app": a, "connected": c, "signedIn": s})).collect::<Vec<_>>(),
    })
}

// ── the box (tests drive a fake) ─────────────────────────────────────

pub trait Ops {
    fn linux(&self) -> bool;
    fn apps_on(&self) -> bool;
    /// `GET /cli/app/box`'s doctor.
    fn doctor(&self) -> J;
    fn mem_total_bytes(&self) -> Option<u64>;
    /// The shared root latch (`root_helper::busy`); `false` = held.
    fn begin_busy(&self) -> bool;
    fn end_busy(&self);
    /// The owner's prepare through the root helper (`box_prep::prepare_core`).
    fn prepare(&self, opts: &PrepareOpts, pod: &PodmanOpts) -> Result<J, String>;
    fn install_engine(&self) -> Result<J, String>;
    /// Start the graceful restart (systemd brings the daemon back).
    fn restart_now(&self) -> Result<(), String>;
    /// Wake the restart timer (a schedule changed).
    fn restart_schedule_changed(&self);
    /// The owner's `ownerTimezone` setting.
    fn owner_zone(&self) -> Option<String>;
    /// `(connected, signed in)` guests per App.
    fn guests(&self) -> (Vec<(String, u64)>, Vec<(String, u64)>);
    fn now(&self) -> i64;
    fn audit(&self, event: &str, actor: &str, detail: &str);
}

// ── routes ───────────────────────────────────────────────────────────

/// `GET /cli/app/box/ready-offer`: the card for an owner, `{owner: false}`
/// for anyone else.
pub(crate) fn status_with(ops: &dyn Ops, store: &Store, who: &Who) -> R<J> {
    need_credential(who)?;
    if !is_owner(who) {
        return Ok(json!({ "owner": false, "offer": null }));
    }
    let s = store.load();
    let linux = ops.linux();
    let apps_on = ops.apps_on();
    let doctor = if linux && apps_on { Some(ops.doctor()) } else { None };
    let ready = doctor.as_ref().is_some_and(|d| d["ready"] == true);
    let f = Facts { linux, apps_on, ready };
    let restart_needed = doctor.as_ref().is_some_and(restart_needed);
    let mut out = json!({
        "owner": true,
        "linux": linux,
        "apps": apps_on,
        "ready": ready,
        "offer": offer(&f, &s),
        "run": s.run,
        "restartNeeded": restart_needed,
        "restart": s.restart,
        "tonightHour": TONIGHT_HOUR,
        "coresMemMinMib": ops.mem_total_bytes().and_then(k2_core::agents_leaf::cores_floor_mib),
    });
    if let Some(d) = &doctor {
        out["blocking"] = json!(blocking_rows(d));
    }
    if restart_needed || s.restart.is_some() {
        let (c, si) = ops.guests();
        out["guests"] = guests_json(&c, &si);
    }
    Ok(out)
}

/// `POST …/trigger {trigger: "apps_page"}`: the owner opened the Apps page.
/// Anyone else: recorded nothing.
pub(crate) fn trigger_with(ops: &dyn Ops, store: &Store, who: &Who, body: &J) -> R<J> {
    need_credential(who)?;
    let t = body.get("trigger").and_then(J::as_str).unwrap_or("");
    if t != TRIGGER_APPS_PAGE {
        return Err(no("bad_request", 400, "trigger is apps_page (the others are the daemon's)"));
    }
    if !is_owner(who) {
        return Ok(json!({ "owner": false, "recorded": false, "offer": null }));
    }
    let now = ops.now();
    let new = store.update(|s| note_trigger(s, TRIGGER_APPS_PAGE, now)).map_err(|e| no("state_write_failed", 500, e))?;
    let mut out = status_with(ops, store, who)?;
    out["recorded"] = json!(new);
    Ok(out)
}

/// `POST …/dismiss {trigger}`: "Not now" (or the card closed).
pub(crate) fn dismiss_with(ops: &dyn Ops, store: &Store, who: &Who, body: &J) -> R<J> {
    owner_only(who, "answering the Apps setup card")?;
    let key = body.get("trigger").and_then(J::as_str).map(str::trim).filter(|k| !k.is_empty());
    let Some(key) = key else {
        return Err(no("bad_request", 400, "trigger is required (the offer's trigger)"));
    };
    let now = ops.now();
    let n = store.update(|s| answer(s, Some(key), "dismissed", now)).map_err(|e| no("state_write_failed", 500, e))?;
    if n == 0 {
        return Err(no("not_found", 404, format!("no waiting offer for trigger {key}")));
    }
    ops.audit("app-ready-offer-dismiss", &super::routes::actor(who), key);
    status_with(ops, store, who)
}

/// The checks before a run, the latch, and `running` recorded. The caller
/// then runs [`run_job`] (a thread in the daemon).
pub(crate) fn begin_run(ops: &dyn Ops, store: &Store, who: &Who) -> R<String> {
    owner_only(who, "getting this server ready for Apps")?;
    if !ops.linux() {
        return Err(no("linux_only", 409, "Apps run on Linux servers only"));
    }
    if store.load().run.is_some_and(|r| r.state == "running") {
        return Err(no("already_running", 409, "the setup is already running"));
    }
    if !ops.begin_busy() {
        return Err(no(
            "root_busy",
            409,
            "another root step (a hosted-mail change, the root helper's update, an automatic prep) is running; try again in a few minutes",
        ));
    }
    let by = super::routes::actor(who);
    let now = ops.now();
    let rec = store.update(|s| {
        answer(s, None, "ran", now);
        s.run = Some(Run { state: "running".into(), phase: "prepare".into(), started_at: now, by: by.clone(), ..Run::default() });
    });
    if let Err(e) = rec {
        ops.end_busy();
        return Err(no("state_write_failed", 500, e));
    }
    ops.audit("app-ready-run", &by, "started");
    Ok(by)
}

/// Releases the latch however the run ends.
struct Release<'a>(&'a dyn Ops);

impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.0.end_busy();
    }
}

fn set_run(store: &Store, f: impl FnOnce(&mut Run)) {
    if let Err(e) = store.update(|s| {
        if let Some(r) = s.run.as_mut() {
            f(r);
        }
    }) {
        k2_core::log_debug!("[apps] ready run state: {e}");
    }
}

/// The run (after [`begin_run`], which took the latch): prepare, the
/// engine, the doctor. Never restarts anything.
pub fn run_job(ops: &dyn Ops, store: &Store, by: &str) {
    let _release = Release(ops);
    let before = ops.doctor();
    // A box that has a cores floor keeps the owner's; else size it by RAM.
    let cores = if row_ok(&before, "coresSlice") { None } else { ops.mem_total_bytes().and_then(k2_core::agents_leaf::cores_floor_mib) };
    let opts = PrepareOpts { runtime: k2_root_proto::apps::OciRuntime::Crun, cores_mem_min_mib: cores, budget: None };
    let pod = PodmanOpts { install: true, dry_run: false, from: None };
    set_run(store, |r| r.cores_mem_min_mib = cores);
    let fail = |phase: &str, e: String| {
        let now = ops.now();
        let d = ops.doctor();
        set_run(store, |r| {
            r.state = "failed".into();
            r.phase = phase.into();
            r.ended_at = Some(now);
            r.error = Some(e.clone());
            r.ready = Some(d["ready"] == true);
            r.blocking = blocking_rows(&d);
            r.restart_needed = restart_needed(&d);
        });
        ops.audit("app-ready-run", by, &format!("failed at {phase}: {e}"));
    };
    let mut changed: Vec<String> = Vec::new();
    match ops.prepare(&opts, &pod) {
        Ok(v) => {
            if v["podman"]["action"] == "installed" {
                changed.push(format!("installed {PRODUCT_NAME}'s Podman {}", v["podman"]["version"].as_str().unwrap_or("")).trim().to_string());
            }
            for s in v["steps"].as_array().cloned().unwrap_or_default() {
                if s["result"]["changed"] != json!(false) {
                    changed.push(s["verb"].as_str().unwrap_or("").to_string());
                }
            }
        }
        Err(e) => return fail("prepare", e),
    }
    let ch = changed.clone();
    set_run(store, |r| {
        r.phase = "engine".into();
        r.changed = ch;
    });
    if !row_ok(&ops.doctor(), "engine") {
        match ops.install_engine() {
            Ok(_) => changed.push("installed the App engine".into()),
            Err(e) => return fail("engine", e),
        }
    }
    set_run(store, |r| {
        r.phase = "check".into();
        r.changed = changed.clone();
    });
    let d = ops.doctor();
    let now = ops.now();
    let ready = d["ready"] == true;
    set_run(store, |r| {
        r.state = "done".into();
        r.phase = "finished".into();
        r.ended_at = Some(now);
        r.ready = Some(ready);
        r.blocking = blocking_rows(&d);
        r.restart_needed = restart_needed(&d);
    });
    ops.audit("app-ready-run", by, &format!("done: {}", if ready { "READY" } else { "NOT READY" }));
}

/// `POST …/restart {when: "now"|"tonight"|"cancel", tz?}`.
pub(crate) fn restart_with(ops: &dyn Ops, store: &Store, who: &Who, body: &J) -> R<J> {
    owner_only(who, &format!("restarting {PRODUCT_NAME} for the Apps setup"))?;
    let by = super::routes::actor(who);
    let now = ops.now();
    match body.get("when").and_then(J::as_str).unwrap_or("") {
        "now" => {
            store.update(|s| s.restart = None).map_err(|e| no("state_write_failed", 500, e))?;
            ops.restart_now().map_err(|e| no("restart_unavailable", 409, e))?;
            ops.audit("app-ready-restart", &by, "now");
            Ok(json!({ "ok": true, "restarting": true }))
        }
        "tonight" => {
            let tz = body.get("tz").and_then(J::as_str).map(str::to_string).filter(|t| !t.trim().is_empty()).or_else(|| ops.owner_zone());
            let (at, zone) = next_local_hour(now, tz.as_deref(), TONIGHT_HOUR).map_err(|e| no("bad_request", 400, e))?;
            let r = Restart { at, zone: zone.clone(), scheduled_at: now, by: by.clone() };
            store.update(|s| s.restart = Some(r)).map_err(|e| no("state_write_failed", 500, e))?;
            ops.restart_schedule_changed();
            ops.audit("app-ready-restart", &by, &format!("scheduled at {at} ({zone})"));
            status_with(ops, store, who)
        }
        "cancel" => {
            store.update(|s| s.restart = None).map_err(|e| no("state_write_failed", 500, e))?;
            ops.restart_schedule_changed();
            ops.audit("app-ready-restart", &by, "cancelled");
            status_with(ops, store, who)
        }
        _ => Err(no("bad_request", 400, "when is now, tonight or cancel")),
    }
}

/// `GET /cli/app/sessions/live`: guests connected (a socket open now) and
/// signed in (a login session that hasn't ended), per App. Counts only.
pub(crate) fn sessions_live_with(ops: &dyn Ops, who: &Who) -> R<J> {
    owner_only(who, "reading who is signed in to the server's Apps")?;
    let (c, s) = ops.guests();
    Ok(guests_json(&c, &s))
}

/// The boot (the daemon, once): the `upgrade` trigger, a done restart
/// cleared, an unfinished run marked failed.
pub fn boot_with(store: &Store, now: i64) -> Result<(), String> {
    store.update(|s| on_boot(s, now))
}

/// A tier reading: a paid plan starting is a trigger.
pub fn tier_with(store: &Store, tier: Option<&str>, now: i64) -> Result<Option<String>, String> {
    store.update(|s| on_tier(s, tier, now))
}

// ── the live daemon ──────────────────────────────────────────────────

/// The graceful restart (the same as `POST /cli/daemon/restart`).
static RESTARTER: OnceLock<tokio::sync::broadcast::Sender<()>> = OnceLock::new();

/// `daemon_main`: hand over the shutdown trigger.
pub fn install_restarter(tx: tokio::sync::broadcast::Sender<()>) {
    let _ = RESTARTER.set(tx);
}

struct Wake {
    flag: Mutex<bool>,
    cv: Condvar,
}

static WAKE: Wake = Wake { flag: Mutex::new(false), cv: Condvar::new() };

fn wake() {
    *WAKE.flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
    WAKE.cv.notify_all();
}

fn sleep_or_wake(secs: Option<u64>) {
    let deadline = secs.map(|s| Instant::now() + Duration::from_secs(s));
    let mut woke = WAKE.flag.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        if *woke {
            *woke = false;
            return;
        }
        woke = match deadline {
            None => WAKE.cv.wait(woke).unwrap_or_else(|p| p.into_inner()),
            Some(d) => {
                let left = d.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return;
                }
                WAKE.cv.wait_timeout(woke, left).unwrap_or_else(|p| p.into_inner()).0
            }
        };
    }
}

fn fire_restart() -> Result<(), String> {
    let tx = RESTARTER
        .get()
        .ok_or_else(|| format!("this {PRODUCT_NAME} can't restart itself here: run k2 daemon restart (or sudo systemctl restart k2-daemon)"))?;
    let tx = tx.clone();
    // Let the answer reach the app first.
    std::thread::Builder::new()
        .name("apps-ready-restart".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            k2_core::log_debug!("[apps] restarting for the Apps setup (agents leaf)");
            let _ = tx.send(());
        })
        .map(|_| ())
        .map_err(|e| format!("restart thread: {e}"))
}

struct LiveOps;

impl Ops for LiveOps {
    fn linux(&self) -> bool {
        cfg!(target_os = "linux")
    }
    fn apps_on(&self) -> bool {
        k2_core::deploy::enabled()
    }
    fn doctor(&self) -> J {
        super::box_prep::doctor(&super::box_prep::gather())
    }
    fn mem_total_bytes(&self) -> Option<u64> {
        std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| k2_core::agents_leaf::mem_total(&m))
    }
    fn begin_busy(&self) -> bool {
        crate::root_helper::busy::begin("the Apps setup")
    }
    fn end_busy(&self) {
        crate::root_helper::busy::end();
    }
    fn prepare(&self, opts: &PrepareOpts, pod: &PodmanOpts) -> Result<J, String> {
        super::box_prep::prepare_core(opts, pod, ACTOR).map_err(|r| {
            let extra = r.extra.as_ref();
            let step = extra.and_then(|e| e["failedStep"].as_str()).unwrap_or("").to_string();
            if step.is_empty() {
                return format!("{}: {}", r.code, r.hint);
            }
            let why = extra
                .and_then(|e| e["steps"].as_array())
                .and_then(|s| s.last())
                .and_then(|s| s["error"].as_str())
                .unwrap_or(&r.hint)
                .to_string();
            format!("box prep stopped at `{step}`: {why}")
        })
    }
    fn install_engine(&self) -> Result<J, String> {
        super::engine_install::install_core(None).map_err(|r| format!("{}: {}", r.code, r.hint))
    }
    fn restart_now(&self) -> Result<(), String> {
        fire_restart()
    }
    fn restart_schedule_changed(&self) {
        wake();
    }
    fn owner_zone(&self) -> Option<String> {
        k2_core::heartbeats::when::owner_schedule_zone(k2_core::app_settings::load().owner_timezone.as_deref())
    }
    fn guests(&self) -> (Vec<(String, u64)>, Vec<(String, u64)>) {
        let signed_in = k2_core::skin_live_counts::signed_in_guests_by_app(k2_core::deploy::now()).unwrap_or_else(|e| {
            k2_core::log_debug!("[apps] signed-in guests: {e}");
            Vec::new()
        });
        (crate::access_live::connected_guests_by_app(), signed_in)
    }
    fn now(&self) -> i64 {
        k2_core::deploy::now()
    }
    fn audit(&self, event: &str, actor: &str, detail: &str) {
        super::grace::audit(event, actor, detail);
    }
}

pub(crate) fn status_route(who: &Who) -> R<J> {
    status_with(&LiveOps, &Store::live(), who)
}

pub(crate) fn trigger_route(who: &Who, body: &J) -> R<J> {
    trigger_with(&LiveOps, &Store::live(), who, body)
}

pub(crate) fn dismiss_route(who: &Who, body: &J) -> R<J> {
    dismiss_with(&LiveOps, &Store::live(), who, body)
}

pub(crate) fn run_route(who: &Who) -> R<J> {
    let store = Store::live();
    let by = begin_run(&LiveOps, &store, who)?;
    let spawned = std::thread::Builder::new().name("apps-ready-run".into()).spawn(move || {
        let store = Store::live();
        if std::panic::catch_unwind(|| run_job(&LiveOps, &store, &by)).is_err() {
            set_run(&store, |r| {
                r.state = "failed".into();
                r.error = Some("the setup stopped unexpectedly; run it again".into());
                r.ended_at = Some(k2_core::deploy::now());
            });
        }
    });
    if let Err(e) = spawned {
        LiveOps.end_busy();
        set_run(&store, |r| {
            r.state = "failed".into();
            r.error = Some(format!("couldn't start: {e}"));
        });
        return Err(no("run_failed", 500, format!("couldn't start the setup: {e}")));
    }
    status_with(&LiveOps, &store, who)
}

pub(crate) fn restart_route(who: &Who, body: &J) -> R<J> {
    restart_with(&LiveOps, &Store::live(), who, body)
}

pub(crate) fn sessions_live_route(who: &Who) -> R<J> {
    sessions_live_with(&LiveOps, who)
}

fn on_tier_change(r: &k2_core::tunnel::subdomains::TierReading, _prev: Option<&str>) {
    match tier_with(&Store::live(), r.tier.as_deref(), k2_core::deploy::now()) {
        Ok(Some(key)) => super::grace::audit("app-ready-offer-trigger", ACTOR, &key),
        Ok(None) => {}
        Err(e) => k2_core::log_debug!("[apps] ready offer tier: {e}"),
    }
}

/// Boot (the daemon, once): the triggers, then the restart timer (Linux).
pub fn spawn() {
    if cfg!(test) {
        return;
    }
    if let Err(e) = boot_with(&Store::live(), k2_core::deploy::now()) {
        k2_core::log_debug!("[apps] ready offer boot: {e}");
    }
    k2_core::tunnel::subdomains::add_tier_listener(on_tier_change);
    if !cfg!(target_os = "linux") {
        return;
    }
    let _ = std::thread::Builder::new().name("apps-ready-restart-timer".into()).spawn(|| loop {
        let store = Store::live();
        let wait = match store.load().restart {
            None => None,
            Some(r) => {
                let now = k2_core::deploy::now();
                if r.at <= now {
                    super::grace::audit("app-ready-restart", ACTOR, &format!("tonight's restart (scheduled by {})", r.by));
                    if let Err(e) = fire_restart() {
                        k2_core::log_debug!("[apps] tonight's restart: {e}");
                        let _ = store.update(|s| s.restart = None);
                    }
                    // The process is going down; don't fire twice.
                    Some(3600)
                } else {
                    Some((r.at - now) as u64)
                }
            }
        };
        sleep_or_wake(wait);
    });
}

#[cfg(test)]
#[path = "ready_offer_tests.rs"]
mod tests;
