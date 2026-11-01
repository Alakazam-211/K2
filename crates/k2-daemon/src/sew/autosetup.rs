//! S6 of the voice rooms PRD (v1.9 §7, rulings 10-30): Sew is set up on
//! every Linux box, new installs and upgrades alike, with no card, and
//! started by the Pro tier watch. **Dark in 0.46.8**: all of it runs only
//! with the voice rooms preview on (`voice_rooms_preview` /
//! `K2_VOICE_ROOMS=1`, [`k2_core::voice_preview`]); with it off nothing
//! here touches any box.
//!
//! - **Set up** (§7.2): in the background, by the daemon (the health loop,
//!   at boot and on later ticks): the pinned Sew release fetched and
//!   installed through the root helper, its master key and `sew-launch-key`
//!   in the owner's vault, Sew's own `install` (config, units). Units stay
//!   **stopped** and no database runs ([`EnableRequest::setup_only`]); the
//!   row is `disabled` with `enableProgress.setupOnly`. Status: "Set up ·
//!   waiting for Pro". A failed fetch or helper step never fails anything
//!   else: it is recorded and retried daily ([`SETUP_RETRY_SECS`]).
//! - **Pro** (§7.2, §8): the daemon-owned tier watch reads the cached plan
//!   tier ([`super::ops::SewOps::plan_tier`]; refresh rule at
//!   `k2_core::tunnel::subdomains::TIER_FRESH`). On the first `pro` it
//!   starts Sew: a normal enable resumes after `install` (units, database,
//!   migrations, start, link). Ensuring the Admin Space and Room 1 (lane B)
//!   and installing voice in the tunnel tier (lane A) follow a running Sew.
//!   Losing Pro stops nothing here (rooms close in lane B; Sew stays set up).
//! - **The small-box floor** (§7.3, [`floor`]): under 1 GiB free disk, not
//!   set up; under 2 GiB of RAM, set up but never started, even on Pro;
//!   2–4 GiB, chat and the Space start but voice doesn't
//!   ([`Floor::voice`], read by lane A before it installs voice). Re-checked
//!   at boot and daily.
//! - **Opting out** (§7.2, §7.4): `K2_SEW_DEFAULT=off` before set-up; a
//!   purge records an opt-out so K2 never sets Sew up again on its own. A
//!   box already running Sew (any install a person made) is never touched.
//!
//! What K2 last did lives in `~/.k2/sew/autosetup.json` ([`RECORD`]; no
//! secret), shown as `autoSetup` in `/cli/sew/status`.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use super::store::{self, Row};
use super::supervisor::{self as sup, EnableRequest, Env, GIB};

/// K2's record of the automatic set-up (`~/.k2/sew/autosetup.json`).
pub const RECORD: &str = "autosetup";
/// `K2_SEW_DEFAULT=off`: never set Sew up on this box on K2's own.
pub const OPT_OUT_ENV: &str = "K2_SEW_DEFAULT";
/// A failed set-up (fetch, helper, preflight) is retried this often.
pub const SETUP_RETRY_SECS: i64 = 86_400;
/// A failed Pro start is retried this often.
pub const START_RETRY_SECS: i64 = 3_600;
/// Memory and disk are re-checked this often (and at every boot).
pub const FLOOR_RECHECK_SECS: i64 = 86_400;
/// Under this much free disk Sew is not set up (§7.3).
pub const DISK_FLOOR: u64 = GIB;
/// Under this much RAM Sew is set up but never started (§7.3).
pub const RAM_CHAT: u64 = 2 * GIB;
/// Under this much RAM chat and the Space start, voice doesn't (§7.3).
pub const RAM_VOICE: u64 = 4 * GIB;
/// Where the free-disk floor is measured (Sew's data, `/var/lib/sew`).
pub const DISK_PATH: &str = "/var/lib";

// ── the small-box floor ──────────────────────────────────────────────────

/// What this box's memory and disk allow (§7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Floor {
    /// Sew may be set up (enough free disk).
    pub set_up: bool,
    /// Sew's chat core and the K2 Space may start (enough RAM).
    pub chat: bool,
    /// Voice may be installed and run.
    pub voice: bool,
    /// `ok`, `disk`, `ram_chat`, `ram_voice` or `unreadable`.
    pub code: &'static str,
    /// The status line when something doesn't fit.
    pub line: Option<String>,
}

impl Floor {
    pub fn json(&self) -> Value {
        json!({"setUp": self.set_up, "chat": self.chat, "voice": self.voice, "code": self.code, "line": self.line})
    }
}

/// `400 MB`, `1 GiB`, `3.5 GiB`.
fn size(b: u64) -> String {
    if b < GIB {
        format!("{} MB", b / 1_000_000)
    } else if b % GIB == 0 {
        format!("{} GiB", b / GIB)
    } else {
        format!("{:.1} GiB", b as f64 / GIB as f64)
    }
}

/// The floor for `mem` total RAM and `disk` free on [`DISK_PATH`] (`None`:
/// unreadable, which fits nothing).
pub fn floor(mem: Option<u64>, disk: Option<u64>) -> Floor {
    let (Some(mem), Some(disk)) = (mem, disk) else {
        return Floor {
            set_up: false,
            chat: false,
            voice: false,
            code: "unreadable",
            line: Some("K2 couldn't read this box's memory or free disk; it checks again later.".into()),
        };
    };
    if disk < DISK_FLOOR {
        return Floor {
            set_up: false,
            chat: false,
            voice: false,
            code: "disk",
            line: Some(format!("Voice rooms need {} of free disk; this box has {} free.", size(DISK_FLOOR), size(disk))),
        };
    }
    if mem < RAM_CHAT {
        return Floor {
            set_up: true,
            chat: false,
            voice: false,
            code: "ram_chat",
            line: Some(format!(
                "Voice rooms need {} of memory; this box has {}. They start when it has enough.",
                size(RAM_CHAT),
                size(mem)
            )),
        };
    }
    if mem < RAM_VOICE {
        return Floor {
            set_up: true,
            chat: true,
            voice: false,
            code: "ram_voice",
            line: Some(format!("Voice needs {} of memory; this box has {}.", size(RAM_VOICE), size(mem))),
        };
    }
    Floor { set_up: true, chat: true, voice: true, code: "ok", line: None }
}

/// This box's floor now.
pub fn floor_now(env: &Env) -> Floor {
    floor(env.ops.mem_total(), env.ops.disk_free(DISK_PATH))
}

// ── opting out ───────────────────────────────────────────────────────────

/// Why K2 must not set Sew up here on its own: `env` ([`OPT_OUT_ENV`] is
/// `off`) or what the record says (`purge`).
pub fn opted_out(env: &Env) -> Option<String> {
    if std::env::var(OPT_OUT_ENV).is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "0" | "false" | "no")) {
        return Some("env".into());
    }
    let rec = env.ops.record_load(RECORD)?;
    if rec["optedOut"] == true {
        return Some(rec["optedOutWhy"].as_str().unwrap_or("owner").to_string());
    }
    None
}

/// Record that the owner opted out (`why`: `purge`). Kept across purges:
/// only a person's `k2 hostchat enable` sets Sew up after this.
pub fn opt_out(env: &Env, why: &str) {
    let mut rec = env.ops.record_load(RECORD).unwrap_or_else(|| json!({}));
    rec["optedOut"] = json!(true);
    rec["optedOutWhy"] = json!(why);
    rec["optedOutAt"] = json!(env.ops.now());
    if let Err(e) = env.ops.record_save(RECORD, Some(&rec)) {
        k2_core::log_debug!("[sew] autosetup opt-out record: {e}");
    }
}

// ── which rows are K2's own ─────────────────────────────────────────────

fn progress_flag(row: &Row, key: &str) -> bool {
    row.enable_progress.as_ref().and_then(|p| p.get(key)).and_then(Value::as_bool) == Some(true)
}

/// K2 set this install up on its own and it waits for Pro (never started).
pub fn waiting_for_pro(row: &Row) -> bool {
    row.status == "disabled"
        && progress_flag(row, "auto")
        && progress_flag(row, "setupOnly")
        && row.enable_progress.as_ref().and_then(|p| p.get("result")).and_then(Value::as_str) == Some("set-up")
}

/// K2's own set-up or Pro start that failed (`status = error`), and which.
fn failed_auto(row: &Row) -> Option<Action> {
    if row.status != "error" || !progress_flag(row, "auto") {
        return None;
    }
    Some(if progress_flag(row, "setupOnly") { Action::SetUp } else { Action::Start })
}

// ── the tick ─────────────────────────────────────────────────────────────

/// What the tick wants run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Set Sew up, not started.
    SetUp,
    /// Start a set-up Sew (Pro).
    Start,
}

fn save(env: &Env, mut rec: Value, now: i64) {
    rec["at"] = json!(now);
    // Keep an opt-out the owner made.
    if let Some(old) = env.ops.record_load(RECORD) {
        for k in ["optedOut", "optedOutWhy", "optedOutAt"] {
            if let Some(v) = old.get(k) {
                rec[k] = v.clone();
            }
        }
    }
    if let Err(e) = env.ops.record_save(RECORD, Some(&rec)) {
        k2_core::log_debug!("[sew] autosetup record: {e}");
    }
}

/// Record `state` unless the record already says it (no rewrite, no
/// signal, every minute).
fn note(env: &Env, state: &str, line: Option<String>, next: Option<i64>, now: i64) {
    let rec = env.ops.record_load(RECORD).unwrap_or(Value::Null);
    if rec["state"] == state && rec["line"] == json!(line) && (next.is_none() || rec["nextTryAt"] == json!(next)) {
        return;
    }
    save(env, json!({"state": state, "line": line, "nextTryAt": next}), now);
    sup::changed("autosetup");
}

/// Decide what to run now (`boot`: the first tick of this daemon, which
/// re-checks a floor or a failure without waiting). Records the states
/// that need no run (opted out, below the floor, waiting for Pro). Pure
/// apart from the record; never runs the helper.
pub fn due(env: &Env, now: i64, boot: bool) -> Option<Action> {
    if !env.ops.linux() || !env.ops.voice_preview() {
        return None;
    }
    let row = store::load();
    if let Some(why) = opted_out(env) {
        if row.is_none() {
            note(env, "opted_out", Some(opt_out_line(&why)), None, now);
        }
        return None;
    }
    let rec = env.ops.record_load(RECORD).unwrap_or(Value::Null);
    let waiting = !boot && rec["nextTryAt"].as_i64().is_some_and(|n| now < n);
    let fl = floor_now(env);
    match row {
        None => {
            if waiting {
                return None;
            }
            if !fl.set_up {
                note(env, "below_floor", fl.line.clone(), Some(now + FLOOR_RECHECK_SECS), now);
                return None;
            }
            if !env.ops.sew_released() {
                note(env, "not_released", Some(sup::not_released().hint), Some(now + SETUP_RETRY_SECS), now);
                return None;
            }
            Some(Action::SetUp)
        }
        Some(r) if waiting_for_pro(&r) => {
            if !env.ops.plan_tier().is_pro() {
                note(env, "set_up", Some(SET_UP_LINE.into()), None, now);
                return None;
            }
            if waiting {
                return None;
            }
            if !fl.chat {
                note(env, "below_floor", fl.line.clone(), Some(now + FLOOR_RECHECK_SECS), now);
                return None;
            }
            Some(Action::Start)
        }
        Some(r) => match failed_auto(&r) {
            Some(Action::SetUp) if !waiting && fl.set_up => Some(Action::SetUp),
            Some(Action::Start) if !waiting && fl.chat && env.ops.plan_tier().is_pro() => Some(Action::Start),
            // The owner's own install (or K2's, running): never touched.
            _ => None,
        },
    }
}

/// "Set up · waiting for Pro".
pub const SET_UP_LINE: &str = "Set up · waiting for Pro";

fn opt_out_line(why: &str) -> String {
    match why {
        "env" => format!("Not set up: turned off on this box ({OPT_OUT_ENV}=off)"),
        "purge" => "Not set up: the owner purged Chat / Voice Hosting (k2 hostchat enable sets it up again)".into(),
        other => format!("Not set up: turned off ({other})"),
    }
}

/// Run `action` to its end (the enable machine; minutes). Records the
/// outcome and returns it.
pub fn run(env: &Env, action: Action) -> Value {
    let req = EnableRequest { auto: true, setup_only: action == Action::SetUp, ..Default::default() };
    let retry = if action == Action::SetUp { SETUP_RETRY_SECS } else { START_RETRY_SECS };
    let failed = |why: String| {
        let now = env.ops.now();
        let state = if action == Action::SetUp { "failed" } else { "start_failed" };
        let line = format!(
            "{}: {why}; K2 tries again on its own",
            if action == Action::SetUp { "Not set up yet" } else { "Set up, not started yet" }
        );
        save(env, json!({"state": state, "line": line, "error": why, "nextTryAt": now + retry}), now);
        sup::changed("autosetup");
        json!({"state": state, "error": why})
    };
    let latch = match sup::enable_begin(env, &req) {
        Err(f) => return failed(format!("{}: {}", f.code, f.hint)),
        Ok(Err(_already)) => {
            let now = env.ops.now();
            save(env, json!({"state": "started", "line": STARTED_LINE}), now);
            return json!({"state": "started"});
        }
        Ok(Ok(l)) => l,
    };
    let r = sup::enable_machine(env, &req);
    drop(latch);
    if let Err(e) = r {
        return failed(e);
    }
    let now = env.ops.now();
    let (state, line) = if action == Action::SetUp { ("set_up", SET_UP_LINE) } else { ("started", STARTED_LINE) };
    save(env, json!({"state": state, "line": line}), now);
    sup::changed("autosetup");
    k2_core::log_debug!("[sew] autosetup: {state}");
    // Voice rooms (lane B): a Pro start is followed by the K2 Admin Space
    // and Room 1, which the grant-hook sweep ensures; ask for it now.
    if action == Action::Start {
        k2_core::tool_hook::kick_reconcile();
    }
    json!({"state": state})
}

/// Sew started because the box is Pro.
pub const STARTED_LINE: &str = "Started for voice rooms (Pro)";

static RUNNING: AtomicBool = AtomicBool::new(false);
static BOOTED: AtomicBool = AtomicBool::new(false);

/// The health loop's step (every 60 s): decide, then run in the background
/// (one run at a time; the supervisor's latch also keeps it off any
/// lifecycle step a person started).
pub fn tick(env: Env<'static>) {
    let boot = !BOOTED.swap(true, Ordering::SeqCst);
    let Some(action) = due(&env, env.ops.now(), boot) else { return };
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new().name("sew-autosetup".into()).spawn(move || {
        let out = run(&env, action);
        k2_core::log_debug!("[sew] autosetup {action:?}: {out}");
        RUNNING.store(false, Ordering::SeqCst);
    });
    if let Err(e) = spawned {
        RUNNING.store(false, Ordering::SeqCst);
        k2_core::log_debug!("[sew] autosetup: no thread: {e}");
    }
}

/// `autoSetup` in `/cli/sew/status`: shown while the preview is on.
/// `None` otherwise (dark).
pub fn status(env: &Env) -> Option<Value> {
    if !env.ops.linux() || !env.ops.voice_preview() {
        return None;
    }
    let rec = env.ops.record_load(RECORD).unwrap_or_else(|| json!({}));
    let fl = floor_now(env);
    let tier = env.ops.plan_tier();
    let row = store::load();
    let mut line = rec.get("line").and_then(Value::as_str).map(str::to_string);
    let mut state = rec.get("state").and_then(Value::as_str).unwrap_or("none").to_string();
    if row.as_ref().is_some_and(waiting_for_pro) && !tier.is_pro() {
        state = "set_up".into();
        line = Some(SET_UP_LINE.into());
    }
    // On Pro with 2–4 GiB: chat runs, voice doesn't; say so.
    if state == "started" && !fl.voice {
        line = fl.line.clone();
    }
    Some(json!({
        "preview": env.ops.voice_preview(),
        "state": state,
        "line": line,
        "error": rec.get("error"),
        "nextTryAt": rec.get("nextTryAt"),
        "optedOut": rec.get("optedOut").and_then(Value::as_bool).unwrap_or(false),
        "floor": fl.json(),
        "tier": super::pro_gate::hosting(env),
    }))
}

/// The tier hook (`k2_core::tunnel::subdomains::set_tier_hook`): a tier
/// change is a refetch signal for status, and the next tick acts on it.
pub fn on_tier_change(r: &k2_core::tunnel::subdomains::TierReading, prev: Option<&str>) {
    if !k2_core::voice_preview::enabled() {
        return;
    }
    k2_core::log_debug!("[sew] plan tier {:?} → {:?} ({})", prev, r.tier, r.primary);
    sup::changed("tier");
}

// Server-only (Linux boxes): drives Sew hosting through the fake box.
#[cfg(test)]
#[cfg(unix)]
#[path = "autosetup_tests.rs"]
mod tests;
