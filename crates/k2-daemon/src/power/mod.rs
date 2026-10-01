//! The daemon's power layer: keep the machine awake while heartbeats
//! fire, and wake it for the next one.
//!
//! Heartbeat S2 (`prd-heartbeat-firing-v1.md` D1, D2, D8, D11–D14;
//! `research-heartbeat-wake-orca-v1.md` W2, W3, W5–W8;
//! `research-heartbeat-wake-privilege-v1.md` §5). Daemon-first: works
//! headless, one truth for every client.
//!
//! - **Awake holds (W2).** [`hold`] returns a guard; while any guard is
//!   alive the daemon holds ONE OS sleep assertion. No admin anywhere:
//!   macOS IOKit `PreventUserIdleSystemSleep` + `PreventSystemSleep`
//!   (dies with the process), Linux a logind inhibitor via
//!   `systemd-inhibit` (released when its pipe closes), Windows
//!   `PowerSetRequest(SystemRequired)`.
//! - **One wake event (W3).** With "Wake this computer for heartbeats"
//!   on, the daemon keeps exactly one OS wake at the earliest next fire
//!   minus 60 s ([`plan_wake`]), re-planned after every due scan and
//!   settings change, cleared when nothing is due. macOS goes through
//!   the root helper ([`helper`], one admin dialog, D11); Linux uses a
//!   `CLOCK_BOOTTIME_ALARM` timerfd (needs `CAP_WAKE_ALARM` from the
//!   Arch package hook, W5); Windows a resume waitable timer (no admin;
//!   the power plan must allow wake timers — K2 reports it and shows the
//!   steps, D14).
//! - **Battery (D12).** Off by default: no wake while on battery. With
//!   "Also on battery" on, still none below 20 %.
//!
//! Every OS call sits behind [`PowerOs`], so tests use [`fake::FakePowerOs`]
//! and never touch real power settings. A daemon started with
//! `K2_HEARTBEAT_NO_SELF_HEAL=1` (test harnesses, scratch HOMEs) gets a
//! no-op backend.

pub mod fake;
pub mod helper;
pub mod parse;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};

/// W3 — the wake fires this long before the heartbeat it is for.
pub const WAKE_LEAD_SECS: i64 = 60;
/// The soonest a wake may be set (the macOS helper clamps the same).
pub const MIN_WAKE_AHEAD_SECS: i64 = 30;

/// AC or battery, as best the OS says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerSource {
    /// `Some(false)` = on battery. `None` = unknown (desktop, server).
    pub on_ac: Option<bool>,
    pub battery_percent: Option<u8>,
}

/// Can this machine schedule a wake at all?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeSupport {
    Ready,
    /// Why not, in words a person can act on.
    Unavailable(String),
}

/// The thin per-OS layer. Each method is one OS call or a read.
pub trait PowerOs: Send + Sync {
    /// Take an OS sleep assertion. Dropping the returned value releases it.
    fn hold_awake(&self, reason: &str) -> Result<Box<dyn Send>, String>;
    fn wake_support(&self) -> WakeSupport;
    /// Replace any K2 wake event with one at `at`.
    fn set_wake(&self, at: DateTime<Utc>) -> Result<(), String>;
    /// Remove any K2 wake event.
    fn clear_wake(&self) -> Result<(), String>;
    fn power_source(&self) -> PowerSource;
    /// D11 — does turning wake on need the one-time admin approval
    /// first? macOS without the helper only.
    fn wake_needs_approval(&self) -> bool {
        false
    }
    /// macOS: run the one admin dialog that installs the helper (D11).
    /// Other OSes need nothing.
    fn install_wake_helper(&self) -> Result<(), String> {
        Ok(())
    }
    /// Extra facts for the status route (Windows wake-timer setting).
    fn notes(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

/// A backend that touches nothing (test daemons, opted-out daemons).
pub struct NoopPowerOs {
    why: String,
}

impl PowerOs for NoopPowerOs {
    fn hold_awake(&self, _reason: &str) -> Result<Box<dyn Send>, String> {
        Ok(Box::new(()))
    }
    fn wake_support(&self) -> WakeSupport {
        WakeSupport::Unavailable(self.why.clone())
    }
    fn set_wake(&self, _at: DateTime<Utc>) -> Result<(), String> {
        Err(self.why.clone())
    }
    fn clear_wake(&self) -> Result<(), String> {
        Ok(())
    }
    fn power_source(&self) -> PowerSource {
        PowerSource::default()
    }
}

fn default_os() -> Arc<dyn PowerOs> {
    let opted_out = std::env::var(k2_core::heartbeats::install::NO_SELF_HEAL_ENV)
        .map(|v| v.trim() == "1")
        .unwrap_or(false);
    // Integration-test binaries live in `target/<profile>/deps/`; they
    // must never take a real assertion or schedule a real wake (HB2's
    // rule, applied to power). Unit tests are `cfg(test)`.
    let test_binary = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(|d| d.file_name()).map(|n| n == "deps"))
        .unwrap_or(false);
    if opted_out || test_binary || cfg!(test) {
        return Arc::new(NoopPowerOs {
            why: "power control is off for this daemon (K2_HEARTBEAT_NO_SELF_HEAL=1)".to_string(),
        });
    }
    #[cfg(target_os = "macos")]
    return Arc::new(macos::MacPowerOs);
    #[cfg(target_os = "linux")]
    return Arc::new(linux::LinuxPowerOs::new());
    #[cfg(windows)]
    return Arc::new(windows::WindowsPowerOs::new());
    #[allow(unreachable_code)]
    Arc::new(NoopPowerOs { why: "no power control on this platform".to_string() })
}

// ── Pure planning ──────────────────────────────────────────────────────

/// What the wake planner decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakePlan {
    /// `off` (switch off), `ok`, `unavailable` (can't on this machine),
    /// `paused` (on battery, D12).
    pub state: &'static str,
    pub reason: String,
    /// Where the one OS wake should be; `None` = no wake event.
    pub target: Option<DateTime<Utc>>,
}

/// D12 — may the machine be woken on this power source?
pub fn battery_allows_wake(on_battery_allowed: bool, src: PowerSource) -> Result<(), String> {
    if src.on_ac != Some(false) {
        return Ok(());
    }
    if !on_battery_allowed {
        return Err("on battery power; wake waits for AC (turn on \"Also on battery\")".to_string());
    }
    match src.battery_percent {
        Some(p) if p < k2_core::app_settings::WAKE_BATTERY_FLOOR_PERCENT => Err(format!(
            "battery at {p}%; no wake below {}%",
            k2_core::app_settings::WAKE_BATTERY_FLOOR_PERCENT
        )),
        _ => Ok(()),
    }
}

/// W3 — the earliest next fire minus 60 s, never sooner than 30 s from
/// now. `None` when nothing is ahead.
pub fn wake_target(next_fires: &[DateTime<Utc>], now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let earliest = next_fires.iter().min()?;
    let target = *earliest - chrono::Duration::seconds(WAKE_LEAD_SECS);
    let floor = now + chrono::Duration::seconds(MIN_WAKE_AHEAD_SECS);
    Some(if target < floor { floor } else { target })
}

/// The whole decision, pure.
pub fn plan_wake(
    ws: &k2_core::app_settings::WakeSchedulerSettings,
    next_fires: &[DateTime<Utc>],
    support: &WakeSupport,
    src: PowerSource,
    now: DateTime<Utc>,
) -> WakePlan {
    if !ws.wake_wanted() {
        return WakePlan {
            state: "off",
            reason: "this computer is not woken for heartbeats; they fire whenever it is awake".into(),
            target: None,
        };
    }
    if let WakeSupport::Unavailable(why) = support {
        return WakePlan { state: "unavailable", reason: why.clone(), target: None };
    }
    if let Err(why) = battery_allows_wake(ws.wake_on_battery, src) {
        return WakePlan { state: "paused", reason: why, target: None };
    }
    match wake_target(next_fires, now) {
        Some(t) => WakePlan { state: "ok", reason: "wake scheduled for the next heartbeat".into(), target: Some(t) },
        None => WakePlan { state: "ok", reason: "no heartbeat is scheduled; no wake set".into(), target: None },
    }
}

/// Next fire of every enabled heartbeat on this daemon (W3; in memory
/// until S3 stores it).
pub fn next_fires(now: chrono::DateTime<chrono::Local>) -> Vec<DateTime<Utc>> {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::AgentHeartbeat::list_all_enabled(&conn).unwrap_or_default()
    };
    rows.iter()
        .filter_map(|hb| k2_core::heartbeats::cron::next_fire_estimate(hb, now))
        .map(|t| t.with_timezone(&Utc))
        .collect()
}

// ── The manager ────────────────────────────────────────────────────────

/// What the OS currently has from us.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Applied {
    /// Not known since boot (a macOS event may survive a reboot).
    Unknown,
    None,
    At(DateTime<Utc>),
}

struct Holds {
    next_id: u64,
    active: BTreeMap<u64, String>,
    os_guard: Option<Box<dyn Send>>,
    last_error: Option<String>,
}

struct WakeState {
    applied: Applied,
    plan: Option<WakePlan>,
    next_fire: Option<DateTime<Utc>>,
    last_error: Option<String>,
    source: PowerSource,
}

pub struct Power {
    os: Arc<dyn PowerOs>,
    holds: Mutex<Holds>,
    wake: Mutex<WakeState>,
}

impl Power {
    pub fn new(os: Arc<dyn PowerOs>) -> Self {
        Self {
            os,
            holds: Mutex::new(Holds { next_id: 1, active: BTreeMap::new(), os_guard: None, last_error: None }),
            wake: Mutex::new(WakeState {
                applied: Applied::Unknown,
                plan: None,
                next_fire: None,
                last_error: None,
                source: PowerSource::default(),
            }),
        }
    }

    /// Take a keep-awake hold. The first hold takes the OS assertion; the
    /// last one dropped releases it.
    pub fn hold(self: &Arc<Self>, reason: &str) -> AwakeHold {
        let mut h = self.holds.lock();
        let id = h.next_id;
        h.next_id += 1;
        h.active.insert(id, reason.to_string());
        if h.os_guard.is_none() {
            match self.os.hold_awake("K2 heartbeat") {
                Ok(g) => {
                    h.os_guard = Some(g);
                    h.last_error = None;
                }
                Err(e) => {
                    k2_core::log_debug!("[power] keep-awake not held: {e}");
                    h.last_error = Some(e);
                }
            }
        }
        AwakeHold { power: Arc::clone(self), id }
    }

    fn release(&self, id: u64) {
        let guard = {
            let mut h = self.holds.lock();
            h.active.remove(&id);
            if h.active.is_empty() {
                h.os_guard.take()
            } else {
                None
            }
        };
        drop(guard); // release outside the lock
    }

    /// Re-plan the one wake event and apply it if it changed.
    pub fn replan(&self, ws: &k2_core::app_settings::WakeSchedulerSettings, fires: &[DateTime<Utc>], now: DateTime<Utc>) -> WakePlan {
        let support = self.os.wake_support();
        let src = self.os.power_source();
        let plan = plan_wake(ws, fires, &support, src, now);
        let mut w = self.wake.lock();
        w.source = src;
        w.next_fire = fires.iter().min().copied();
        let want = match plan.target {
            Some(t) => Applied::At(t),
            None => Applied::None,
        };
        if w.applied != want {
            let result = match (&want, &w.applied) {
                (Applied::At(t), _) => self.os.set_wake(*t),
                // Clear only what we may have set: a known event, or an
                // unknown state on a machine that can schedule at all.
                (Applied::None, Applied::At(_)) => self.os.clear_wake(),
                (Applied::None, Applied::Unknown) if support == WakeSupport::Ready => self.os.clear_wake(),
                (Applied::None, _) => Ok(()),
                (Applied::Unknown, _) => Ok(()),
            };
            match result {
                Ok(()) => {
                    w.applied = want;
                    w.last_error = None;
                }
                Err(e) => {
                    k2_core::log_debug!("[power] wake event update failed: {e}");
                    w.last_error = Some(e);
                }
            }
        }
        w.plan = Some(plan.clone());
        plan
    }

    /// W8 — what is really held and scheduled, for `scheduler-status`.
    pub fn status_json(&self) -> serde_json::Value {
        let (held, reasons, hold_error) = {
            let h = self.holds.lock();
            let mut reasons: Vec<String> = h.active.values().cloned().collect();
            reasons.dedup();
            (h.os_guard.is_some(), reasons, h.last_error.clone())
        };
        let w = self.wake.lock();
        let support = self.os.wake_support();
        let (state, reason) = match &w.plan {
            Some(p) => (p.state, p.reason.clone()),
            None => ("off", "not planned yet".to_string()),
        };
        serde_json::json!({
            "wake": {
                "state": state,
                "reason": reason,
                "nextWakeAt": match &w.applied { Applied::At(t) => Some(t.to_rfc3339()), _ => None },
                "nextFireAt": w.next_fire.map(|t| t.to_rfc3339()),
                "support": match &support {
                    WakeSupport::Ready => serde_json::json!({ "ready": true }),
                    WakeSupport::Unavailable(why) => serde_json::json!({ "ready": false, "reason": why }),
                },
                "lastError": w.last_error,
                "powerSource": w.source,
                "batteryFloorPercent": k2_core::app_settings::WAKE_BATTERY_FLOOR_PERCENT,
                "platform": std::env::consts::OS,
                "notes": self.os.notes(),
            },
            "awake": {
                "held": held,
                "reasons": reasons,
                "lastError": hold_error,
            },
        })
    }
}

/// A keep-awake hold. Dropping it releases (the OS assertion goes when
/// the last hold goes).
pub struct AwakeHold {
    power: Arc<Power>,
    id: u64,
}

impl Drop for AwakeHold {
    fn drop(&mut self) {
        self.power.release(self.id);
    }
}

static POWER: RwLock<Option<Arc<Power>>> = RwLock::new(None);

/// The daemon's power manager (created on first use).
pub fn power() -> Arc<Power> {
    if let Some(p) = POWER.read().as_ref() {
        return Arc::clone(p);
    }
    let mut w = POWER.write();
    Arc::clone(w.get_or_insert_with(|| Arc::new(Power::new(default_os()))))
}

/// Swap the OS layer (tests).
#[allow(dead_code)]
pub fn install_os(os: Arc<dyn PowerOs>) -> Arc<Power> {
    let p = Arc::new(Power::new(os));
    *POWER.write() = Some(Arc::clone(&p));
    p
}

/// W2 — keep the machine awake until the guard drops.
pub fn hold(reason: &str) -> AwakeHold {
    power().hold(reason)
}

/// Keep the machine awake for `d`, then release. Runtime-agnostic.
pub fn hold_for(reason: &str, d: Duration) {
    let guard = hold(reason);
    std::thread::spawn(move || {
        std::thread::sleep(d);
        drop(guard);
    });
}

/// Re-plan the wake from saved settings and the current heartbeats.
pub fn replan_wake_now() -> WakePlan {
    let ws = k2_core::app_settings::load().wake_scheduler;
    let fires = next_fires(chrono::Local::now());
    power().replan(&ws, &fires, Utc::now())
}

/// Async wrapper for the monitor loop.
pub async fn replan_wake(context: &'static str) {
    match tokio::task::spawn_blocking(replan_wake_now).await {
        Ok(plan) => {
            if plan.target.is_some() || plan.state != "off" {
                k2_core::log_debug!(
                    "[power] wake plan ({context}): {} — {} {:?}",
                    plan.state,
                    plan.reason,
                    plan.target
                );
            }
        }
        Err(e) => k2_core::log_debug!("[power] wake replan join error ({context}): {e}"),
    }
}

/// Boot: first plan, which also clears a stale K2 wake event left by a
/// previous run (macOS events survive a reboot).
pub async fn boot() {
    replan_wake("boot").await;
}

/// D11 — the approval step of turning wake on. Shows the one admin
/// dialog only when the OS layer needs it (macOS, helper missing). A
/// decline keeps the switch off and says why. Returns
/// `(switch_on, message)`; the message is empty when nothing happened.
pub fn approve_wake(os: &dyn PowerOs, enabled: bool) -> (bool, String) {
    if !enabled || !os.wake_needs_approval() {
        return (enabled, String::new());
    }
    match os.install_wake_helper() {
        Ok(()) => (true, "Helper installed.".to_string()),
        Err(e) => (
            false,
            format!("Wake stays off: it needs a one-time admin approval to install a small helper. {e}"),
        ),
    }
}

/// D8 / D11 — turn "Wake this computer for heartbeats" on or off.
/// On macOS without the helper, turning it ON shows the one admin
/// dialog; if the user declines, the switch stays off and the result
/// says why. Returns `(saved_on, message)`.
pub fn set_wake_enabled(enabled: bool, on_battery: Option<bool>) -> Result<(bool, String), String> {
    let p = power();
    let (effective, mut message) = approve_wake(p.os.as_ref(), enabled);
    let mut partial = serde_json::json!({ "wakeScheduler": { "wakeForHeartbeats": effective } });
    if let Some(b) = on_battery {
        partial["wakeScheduler"]["wakeOnBattery"] = serde_json::Value::Bool(b);
    }
    k2_core::app_settings::update(partial)?;
    let plan = replan_wake_now();
    if message.is_empty() {
        message = plan.reason.clone();
    }
    Ok((effective, message))
}

#[cfg(test)]
mod tests {
    use super::fake::FakePowerOs;
    use super::*;
    use k2_core::app_settings::WakeSchedulerSettings;

    fn ws(on: bool, battery: bool) -> WakeSchedulerSettings {
        WakeSchedulerSettings {
            wake_for_heartbeats: Some(on),
            wake_on_battery: battery,
            ..WakeSchedulerSettings::default()
        }
    }

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).expect("rfc3339").with_timezone(&Utc)
    }

    const AC: PowerSource = PowerSource { on_ac: Some(true), battery_percent: Some(80) };

    /// T-W3 — the earliest next fire minus 60 s.
    #[test]
    fn wake_target_is_earliest_next_fire_minus_60s() {
        let now = t("2026-10-01T10:00:00Z");
        let fires = [t("2026-10-01T12:00:00Z"), t("2026-10-01T10:30:00Z"), t("2026-10-02T09:00:00Z")];
        assert_eq!(wake_target(&fires, now), Some(t("2026-10-01T10:29:00Z")));
        assert_eq!(wake_target(&[], now), None);
        // Due within the minute: never in the past, 30 s ahead at least.
        assert_eq!(wake_target(&[t("2026-10-01T10:00:20Z")], now), Some(t("2026-10-01T10:00:30Z")));
    }

    #[test]
    fn plan_respects_switch_support_and_battery() {
        let now = t("2026-10-01T10:00:00Z");
        let fires = [t("2026-10-01T11:00:00Z")];
        let p = plan_wake(&ws(false, false), &fires, &WakeSupport::Ready, AC, now);
        assert_eq!((p.state, p.target), ("off", None));

        let p = plan_wake(&ws(true, false), &fires, &WakeSupport::Unavailable("helper not installed".into()), AC, now);
        assert_eq!((p.state, p.target), ("unavailable", None));
        assert!(p.reason.contains("helper"), "{}", p.reason);

        let p = plan_wake(&ws(true, false), &fires, &WakeSupport::Ready, AC, now);
        assert_eq!((p.state, p.target), ("ok", Some(t("2026-10-01T10:59:00Z"))));

        // D12: battery, no checkbox → paused.
        let batt = PowerSource { on_ac: Some(false), battery_percent: Some(70) };
        let p = plan_wake(&ws(true, false), &fires, &WakeSupport::Ready, batt, now);
        assert_eq!((p.state, p.target), ("paused", None));
        // Checkbox on → allowed above the floor…
        let p = plan_wake(&ws(true, true), &fires, &WakeSupport::Ready, batt, now);
        assert_eq!(p.state, "ok");
        // …but not below 20 %.
        let low = PowerSource { on_ac: Some(false), battery_percent: Some(19) };
        let p = plan_wake(&ws(true, true), &fires, &WakeSupport::Ready, low, now);
        assert_eq!((p.state, p.target), ("paused", None));
        assert!(p.reason.contains("20%"), "{}", p.reason);
        // Unknown source (desktop) counts as AC.
        let p = plan_wake(&ws(true, false), &fires, &WakeSupport::Ready, PowerSource::default(), now);
        assert_eq!(p.state, "ok");
    }

    #[test]
    fn legacy_settings_seed_the_switch() {
        let mut legacy = WakeSchedulerSettings::default();
        assert!(!legacy.wake_wanted());
        legacy.mode = "heartbeat".into();
        legacy.wake_system = true;
        assert!(legacy.wake_wanted(), "old heartbeat+WakeSystem means the user wanted wake");
        legacy.wake_for_heartbeats = Some(false);
        assert!(!legacy.wake_wanted(), "the new switch wins once set");
    }

    /// The manager keeps ONE OS wake event and only calls the OS when the
    /// target changes.
    #[test]
    fn manager_sets_one_wake_and_clears_it() {
        let fake = Arc::new(FakePowerOs::ready());
        let p = Power::new(fake.clone());
        let now = t("2026-10-01T10:00:00Z");
        let fires = [t("2026-10-01T11:00:00Z")];
        p.replan(&ws(true, false), &fires, now);
        p.replan(&ws(true, false), &fires, now); // unchanged → no second call
        assert_eq!(fake.wake_calls(), vec![Some(t("2026-10-01T10:59:00Z"))]);
        p.replan(&ws(true, false), &[t("2026-10-01T10:30:00Z")], now);
        p.replan(&ws(false, false), &fires, now);
        assert_eq!(
            fake.wake_calls(),
            vec![Some(t("2026-10-01T10:59:00Z")), Some(t("2026-10-01T10:29:00Z")), None]
        );
        let s = p.status_json();
        assert_eq!(s["wake"]["state"], "off");
        assert!(s["wake"]["nextWakeAt"].is_null());
    }

    /// Boot with wake off on a machine that can schedule: clear once, in
    /// case a previous run left an event.
    #[test]
    fn boot_clears_a_stale_event_once() {
        let fake = Arc::new(FakePowerOs::ready());
        let p = Power::new(fake.clone());
        let now = t("2026-10-01T10:00:00Z");
        p.replan(&ws(false, false), &[], now);
        p.replan(&ws(false, false), &[], now);
        assert_eq!(fake.wake_calls(), vec![None]);
    }

    /// D11 — one dialog, only when turning wake ON and only when the
    /// helper is missing. A decline keeps the switch off with a reason.
    #[test]
    fn approval_dialog_only_on_first_switch_on() {
        let fake = FakePowerOs::needs_approval(false);
        assert_eq!(approve_wake(&fake, false), (false, String::new()));
        assert_eq!(fake.approval_prompts(), 0, "turning wake off never prompts");
        let (on, why) = approve_wake(&fake, true);
        assert!(!on, "a declined dialog keeps the switch off");
        assert!(why.contains("one-time admin approval"), "{why}");
        assert_eq!(fake.approval_prompts(), 1);

        let accepting = FakePowerOs::needs_approval(true);
        assert_eq!(approve_wake(&accepting, true).0, true);
        assert_eq!(accepting.approval_prompts(), 1);

        let installed = FakePowerOs::ready();
        assert_eq!(approve_wake(&installed, true), (true, String::new()));
        assert_eq!(installed.approval_prompts(), 0, "no dialog once the helper is in");
    }

    /// Without support nothing is called at all (no sudo every minute).
    #[test]
    fn unsupported_machine_never_calls_the_os() {
        let fake = Arc::new(FakePowerOs::unavailable("helper not installed"));
        let p = Power::new(fake.clone());
        p.replan(&ws(true, false), &[t("2026-10-01T11:00:00Z")], t("2026-10-01T10:00:00Z"));
        p.replan(&ws(false, false), &[], t("2026-10-01T10:00:00Z"));
        assert_eq!(fake.wake_calls(), Vec::<Option<DateTime<Utc>>>::new());
    }

    /// T-W2 shape: holds are refcounted onto one OS assertion.
    #[test]
    fn holds_share_one_os_assertion() {
        let fake = Arc::new(FakePowerOs::ready());
        let p = Arc::new(Power::new(fake.clone()));
        let a = p.hold("heartbeat fire");
        let b = p.hold("heartbeat wake scan");
        assert_eq!(fake.holds_taken(), 1);
        assert_eq!(p.status_json()["awake"]["held"], true);
        drop(a);
        assert_eq!(fake.holds_released(), 0, "one hold still alive");
        drop(b);
        assert_eq!(fake.holds_released(), 1);
        assert_eq!(p.status_json()["awake"]["held"], false);
    }
}
