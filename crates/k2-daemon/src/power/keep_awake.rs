//! Heartbeat S6 — the Keep awake control (Off / While agents are
//! working / Always).
//!
//! `prd-heartbeat-firing-v1.md` D10–D14 and the S6 slice note;
//! `research-heartbeat-wake-privilege-v1.md` §5.4. Daemon-owned: the mode
//! is `app_settings.keep_awake`, the loop runs in the daemon, and it holds
//! with no client attached. Clients render [`status_json`].
//!
//! What it holds:
//! - **Lid open, every OS, no admin.** The shared S2 assertion
//!   ([`super::Power::hold`]): IOKit on macOS, a logind inhibitor on
//!   Linux, `PowerSetRequest` on Windows.
//! - **Lid closed, only with "Also with the lid closed" on**
//!   (`prd-power-helper-smappservice-v1.md` S1; research §5.4). Its own
//!   switch, off by default. macOS: the S2 `k2-power-helper` runs `pmset
//!   disablesleep 1` while the mode holds, renewed every minute; its
//!   watcher puts sleep back if the daemon dies or stops renewing. AC only
//!   unless "Also on battery" is on, never below 20 % (D12). Linux: a
//!   logind `handle-lid-switch` lock; headless reports
//!   `limited (no session)` (D13). Windows: no switch; OK only when the
//!   power plan's lid action is "Do nothing"; otherwise K2 shows the
//!   steps (D14).
//! - **No surprise dialog.** Changing the mode or the switch never
//!   installs anything and never shows the admin dialog. Only **Set up**
//!   (`POST /cli/power/helper {action:"setup"}`, or an old client's
//!   `approveLid`) does, and only for an Admin or Owner on the host
//!   itself (loopback), never a Member or a remote client.
//! - **Battery floor.** On battery below 20 % everything pauses.
//!
//! "While agents are working" follows `session_activity_changed`: any
//! session `working` holds; the hold stays [`WORKING_GRACE`] after the
//! last one goes idle. A session stuck `working` past [`WORKING_STALE`]
//! (a lost idle event) stops counting.
//!
//! The status says what is actually held, never just the setting.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_core::app_settings::{KeepAwakeMode, KeepAwakeSettings, WAKE_BATTERY_FLOOR_PERCENT};
use parking_lot::Mutex;

use super::{AwakeHold, HelperCaller, LidAccess, LidFacts, LidRefusal, Power, PowerOs, PowerSource, NO_SESSION_PREFIX};

/// Hold this long after the last working session goes idle.
pub const WORKING_GRACE: Duration = Duration::from_secs(60);
/// A session `working` this long with no idle stops counting.
pub const WORKING_STALE: Duration = Duration::from_secs(2 * 3600);
/// How often the loop re-checks power source, grace and settings.
pub const RECONCILE_INTERVAL: Duration = Duration::from_secs(15);

// ── Which sessions are working ─────────────────────────────────────────

/// Sessions the daemon has seen `working`, keyed `workspace|agent`.
#[derive(Debug, Default)]
pub struct WorkTracker {
    working: HashMap<String, Instant>,
    /// When the working set last became empty.
    went_idle_at: Option<Instant>,
}

impl WorkTracker {
    pub fn key(workspace_path: &str, agent_name: &str) -> String {
        format!("{workspace_path}|{agent_name}")
    }

    /// Apply one `session_activity_changed`. Returns true when this
    /// event started a hold that was not already running (so the caller
    /// re-applies at once instead of waiting for the next tick).
    pub fn on_activity(&mut self, key: &str, status: &str, now: Instant) -> bool {
        let was_busy = self.busy(now);
        if status == "working" {
            self.working.insert(key.to_string(), now);
            self.went_idle_at = None;
        } else {
            self.remove(key, now);
        }
        !was_busy && self.busy(now)
    }

    /// A session went away.
    pub fn on_removed(&mut self, key: &str, now: Instant) {
        self.remove(key, now);
    }

    fn remove(&mut self, key: &str, now: Instant) {
        if self.working.remove(key).is_some() && self.working.is_empty() {
            self.went_idle_at = Some(now);
        }
    }

    /// Any session working, or the grace after the last one still
    /// running. Drops sessions stuck `working` past [`WORKING_STALE`].
    pub fn busy(&mut self, now: Instant) -> bool {
        self.working.retain(|_, since| now.saturating_duration_since(*since) < WORKING_STALE);
        if !self.working.is_empty() {
            return true;
        }
        matches!(self.went_idle_at, Some(t) if now.saturating_duration_since(t) < WORKING_GRACE)
    }

    pub fn working_count(&self) -> usize {
        self.working.len()
    }
}

// ── Pure policy ────────────────────────────────────────────────────────

/// What the policy wants held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The mode asks for a hold right now (Always, or Working + busy).
    pub active: bool,
    /// Paused: on battery below the floor.
    pub paused_battery: bool,
    /// Take the lid-open assertion.
    pub hold_open: bool,
    /// Try the lid-closed hold.
    pub want_lid: bool,
    /// Why the lid-closed part is not wanted (shown to the user).
    pub lid_block: Option<String>,
}

fn on_battery(src: PowerSource) -> bool {
    src.on_ac == Some(false)
}

/// Why lid closed sleeps when the switch is off.
pub const LID_SWITCH_OFF: &str = "\"Also with the lid closed\" is off";
/// Why lid closed sleeps on a Mac without the helper.
pub const LID_NOT_SET_UP: &str = "the power helper is not set up on this Mac";

/// The decision, pure. `allow_battery` is "Also on battery" (D12);
/// `lid_switch` is "Also with the lid closed" (always true where the OS,
/// not K2, decides: Windows).
pub fn plan(
    mode: KeepAwakeMode,
    busy: bool,
    src: PowerSource,
    allow_battery: bool,
    lid_switch: bool,
    lid: &LidFacts,
) -> Plan {
    let active = match mode {
        KeepAwakeMode::Off => false,
        KeepAwakeMode::Working => busy,
        KeepAwakeMode::Always => true,
    };
    let paused_battery = active
        && on_battery(src)
        && matches!(src.battery_percent, Some(p) if p < WAKE_BATTERY_FLOOR_PERCENT);
    let hold_open = active && !paused_battery;
    let lid_block = if !hold_open {
        None
    } else if !lid_switch {
        Some(LID_SWITCH_OFF.to_string())
    } else {
        match &lid.access {
            LidAccess::NeedsApproval => Some(LID_NOT_SET_UP.to_string()),
            LidAccess::Unavailable(why) => Some(why.clone()),
            LidAccess::Ready if lid.ac_only_unless_allowed && on_battery(src) && !allow_battery => Some(
                "on battery, lid closed needs power (or turn on \"Also on battery\")".to_string(),
            ),
            LidAccess::Ready => None,
        }
    };
    Plan { active, paused_battery, hold_open, want_lid: hold_open && lid_block.is_none(), lid_block }
}

/// What the user sees. `state` is a fixed vocabulary:
/// `off`, `waiting`, `paused`, `lid_closed_ok`, `lid_open_only`,
/// `limited`, `error`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shown {
    pub state: &'static str,
    pub label: String,
    pub detail: String,
    /// Something is really held at the OS.
    pub held: bool,
    /// The lid-closed part is really held.
    pub lid_held: bool,
}

/// Facts after applying the plan.
#[derive(Debug, Clone)]
pub struct Applied {
    /// `Ok(())` = the lid-open assertion is held; `Err` = why not.
    /// `None` = not asked for.
    pub open: Option<Result<(), String>>,
    pub lid_held: bool,
    /// The lid hold was tried and refused.
    pub lid_refusal: Option<LidRefusal>,
}

/// Turn the plan and what happened into the honest state, pure.
pub fn describe(mode: KeepAwakeMode, p: &Plan, a: &Applied, src: PowerSource) -> Shown {
    let floor = WAKE_BATTERY_FLOOR_PERCENT;
    let shown = |state, label: String, detail: String, held, lid_held| Shown { state, label, detail, held, lid_held };
    if mode == KeepAwakeMode::Off {
        return shown("off", "Keep awake: off".into(), "This computer sleeps as usual.".into(), false, false);
    }
    if !p.active {
        return shown(
            "waiting",
            "Keep awake: waiting for agents".into(),
            "Not holding. It holds while any agent is working, and for 1 minute after.".into(),
            false,
            false,
        );
    }
    if p.paused_battery {
        return shown(
            "paused",
            format!("Paused: battery below {floor}%"),
            format!("On battery below {floor}%, K2 lets this computer sleep."),
            false,
            false,
        );
    }
    match &a.open {
        Some(Ok(())) => {}
        Some(Err(e)) if e.starts_with(NO_SESSION_PREFIX) => {
            return shown(
                "limited",
                "Keep awake: limited (no session)".into(),
                "No login session on this machine, so the system refused the sleep lock.".into(),
                a.lid_held,
                a.lid_held,
            );
        }
        Some(Err(e)) => {
            return shown("error", "Keep awake: not held".into(), format!("The system refused: {e}"), a.lid_held, a.lid_held);
        }
        None => {
            return shown("error", "Keep awake: not held".into(), "No hold was taken.".into(), false, false);
        }
    }
    if a.lid_held {
        let label = if on_battery(src) {
            "Awake, lid closed OK (on battery)"
        } else {
            "Awake, lid closed OK (on power)"
        };
        let detail = if on_battery(src) {
            format!("Stays awake with the lid closed until the battery reaches {floor}%. Closed in a bag, a laptop can get hot.")
        } else {
            "Stays awake with the lid closed while on power.".to_string()
        };
        return shown("lid_closed_ok", label.into(), detail, true, true);
    }
    if let Some(r) = &a.lid_refusal {
        if r.no_session {
            return shown(
                "limited",
                "Awake: limited (no session)".into(),
                format!("Lid closed will still sleep: no login session, so the lid lock was refused. {}", r.reason),
                true,
                false,
            );
        }
        return shown(
            "lid_open_only",
            "Awake (lid open only)".into(),
            format!("Lid closed will still sleep: {}", r.reason),
            true,
            false,
        );
    }
    let why = p.lid_block.clone().unwrap_or_else(|| "the lid-closed hold is not running".into());
    shown("lid_open_only", "Awake (lid open only)".into(), format!("Lid closed will still sleep: {why}"), true, false)
}

// ── The controller ─────────────────────────────────────────────────────

struct Inner {
    tracker: WorkTracker,
    open: Option<AwakeHold>,
    lid: Option<Box<dyn Send>>,
    lid_refusal: Option<LidRefusal>,
    last: Option<(Shown, KeepAwakeMode, PowerSource, Option<LidAccess>)>,
}

pub struct KeepAwake {
    power: Arc<Power>,
    inner: Mutex<Inner>,
}

impl KeepAwake {
    pub fn new(power: Arc<Power>) -> Self {
        Self {
            power,
            inner: Mutex::new(Inner {
                tracker: WorkTracker::default(),
                open: None,
                lid: None,
                lid_refusal: None,
                last: None,
            }),
        }
    }

    fn os(&self) -> Arc<dyn PowerOs> {
        self.power.os()
    }

    /// One `session_activity_changed`. True = re-apply now.
    pub fn note_activity(&self, key: &str, status: &str, now: Instant) -> bool {
        self.inner.lock().tracker.on_activity(key, status, now)
    }

    pub fn note_removed(&self, key: &str, now: Instant) {
        self.inner.lock().tracker.on_removed(key, now);
    }

    /// Apply the mode: take or drop the holds so the OS matches the
    /// policy, and return what is really held. Blocking (the macOS lid
    /// hold runs `sudo -n`). Never installs anything or shows a dialog.
    pub fn reconcile(&self, settings: &KeepAwakeSettings, allow_battery: bool, now: Instant) -> Shown {
        let mode = settings.mode();
        let os = self.os();
        let lid_switch = lid_switch_on(settings, os.as_ref());
        let mut g = self.inner.lock();
        let busy = g.tracker.busy(now);
        // Off: no OS reads at all, just drop anything held.
        let (src, facts) = if mode == KeepAwakeMode::Off {
            (PowerSource::default(), LidFacts { access: LidAccess::Ready, ac_only_unless_allowed: false })
        } else {
            let src = os.power_source();
            (src, os.lid_facts(src))
        };
        let p = plan(mode, busy, src, allow_battery, lid_switch, &facts);

        // Lid first on the way down, open first on the way up.
        if !p.want_lid {
            let old = g.lid.take();
            drop(old);
            g.lid_refusal = None;
        }
        if p.hold_open {
            if g.open.is_none() {
                g.open = Some(self.power.hold("keep awake"));
            }
        } else {
            g.open = None;
        }
        if p.want_lid && g.lid.is_none() {
            match os.hold_lid_closed("K2 keep awake") {
                Ok(guard) => {
                    g.lid = Some(guard);
                    g.lid_refusal = None;
                }
                Err(r) => {
                    k2_core::log_debug!("[keep-awake] lid-closed hold refused: {}", r.reason);
                    g.lid_refusal = Some(r);
                }
            }
        }

        let open = if g.open.is_some() {
            Some(match self.power.assertion_state() {
                (true, _) => Ok(()),
                (false, Some(e)) => Err(e),
                (false, None) => Err("the sleep assertion is not held".to_string()),
            })
        } else {
            None
        };
        let applied = Applied { open, lid_held: g.lid.is_some(), lid_refusal: g.lid_refusal.clone() };
        let shown = describe(mode, &p, &applied, src);
        let access = if mode == KeepAwakeMode::Off { None } else { Some(facts.access) };
        g.last = Some((shown.clone(), mode, src, access));
        shown
    }

    /// The last reconcile, as the route returns it. `caller` decides
    /// whether this client is offered **Set up**.
    pub fn status_json(&self, settings: &KeepAwakeSettings, allow_battery: bool, caller: HelperCaller) -> serde_json::Value {
        let os = self.os();
        let switch_applies = os.lid_switch_applies();
        let last_access = self.inner.lock().last.as_ref().and_then(|l| l.3.clone());
        // Off reconciles read nothing; the menu still shows whether lid
        // closed is set up. A file check on macOS, no power read.
        let access = match last_access {
            Some(a) => Some(a),
            None if switch_applies => Some(os.lid_facts(PowerSource::default()).access),
            None => None,
        };
        let (setup, setup_detail) = match &access {
            Some(LidAccess::Ready) => ("ready", String::new()),
            Some(LidAccess::NeedsApproval) if caller.may_set_up() => (
                "needs_setup",
                "Set up installs a small helper. macOS asks once for an admin password.".to_string(),
            ),
            Some(LidAccess::NeedsApproval) => ("needs_setup", caller.refusal().to_string()),
            Some(LidAccess::Unavailable(why)) => ("unavailable", why.clone()),
            None => ("unknown", String::new()),
        };
        let can_set_up = setup == "needs_setup" && caller.may_set_up();
        let g = self.inner.lock();
        let (shown, src) = match &g.last {
            Some((s, _, src, _)) => (s.clone(), *src),
            None => (
                describe(
                    KeepAwakeMode::Off,
                    &plan(KeepAwakeMode::Off, false, PowerSource::default(), false, false, &LidFacts {
                        access: LidAccess::Ready,
                        ac_only_unless_allowed: false,
                    }),
                    &Applied { open: None, lid_held: false, lid_refusal: None },
                    PowerSource::default(),
                ),
                PowerSource::default(),
            ),
        };
        serde_json::json!({
            "mode": settings.mode().as_wire(),
            "state": shown.state,
            "label": shown.label,
            "detail": shown.detail,
            "held": shown.held,
            "lidHeld": shown.lid_held,
            "workingSessions": g.tracker.working_count(),
            "powerSource": src,
            "batteryFloorPercent": WAKE_BATTERY_FLOOR_PERCENT,
            "alsoOnBattery": allow_battery,
            // "Also with the lid closed": the saved switch, whether this
            // OS has it at all, and whether it is set up.
            "lidClosed": settings.lid_closed.unwrap_or(false),
            "lidSwitch": switch_applies,
            "lidSetup": setup,
            "lidSetupDetail": setup_detail,
            "canSetUp": can_set_up,
            // 0.43.0/0.43.1 clients (P19): their "Allow lid closed" button
            // shows only for a caller who may set it up.
            "canApproveLid": can_set_up,
            "lidDialogDeclined": false,
            "platform": std::env::consts::OS,
        })
    }
}

/// "Also with the lid closed", effective. Where the OS decides (Windows)
/// there is no switch, so the plan reports the OS setting as before.
pub fn lid_switch_on(settings: &KeepAwakeSettings, os: &dyn PowerOs) -> bool {
    !os.lid_switch_applies() || settings.lid_closed.unwrap_or(false)
}

/// The switch for a settings file that never had one. A 0.43.0 Mac that
/// approved the helper's admin dialog held the lid closed in every mode,
/// so it keeps doing that (switch on). Linux held it in every mode with
/// no approval, so a user who had a mode on keeps it. Everyone else
/// starts off. Off reads no lid facts.
pub fn migrated_lid_switch(mode: KeepAwakeMode, os: &dyn PowerOs) -> bool {
    if !os.lid_switch_applies() {
        return false;
    }
    if os.lid_helper_approved() {
        return true;
    }
    mode != KeepAwakeMode::Off && os.lid_facts(PowerSource::default()).access == LidAccess::Ready
}

/// Boot: save [`migrated_lid_switch`] once, when the file has no switch.
fn resolve_lid_switch_once(os: &dyn PowerOs) {
    let current = k2_core::app_settings::load().keep_awake;
    if current.lid_closed.is_some() {
        return;
    }
    let on = migrated_lid_switch(current.mode(), os);
    match k2_core::app_settings::update(serde_json::json!({ "keepAwake": { "lidClosed": on } })) {
        Ok(_) => k2_core::log_debug!("[keep-awake] \"Also with the lid closed\" set {on} from the old behaviour"),
        Err(e) => k2_core::log_debug!("[keep-awake] could not save the lid-closed switch: {e}"),
    }
}

/// What **Set up** did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetUp {
    /// The helper is in (or nothing was needed).
    pub ok: bool,
    /// The admin dialog ran.
    pub dialog: bool,
    pub message: String,
}

/// Power-helper S1 — the ONLY way Keep awake reaches the installer. Until
/// S3 this is still the 0.43.0 `osascript` dialog; it runs only when the
/// helper is missing and only for a caller who may set it up (a local
/// Admin or Owner). A mode change never comes here.
pub fn set_up(os: &dyn PowerOs, caller: HelperCaller) -> SetUp {
    let needed = os.wake_needs_approval()
        || (os.lid_switch_applies() && os.lid_facts(PowerSource::default()).access == LidAccess::NeedsApproval);
    if !needed {
        let message = if os.lid_helper_approved() {
            "The power helper is already set up."
        } else {
            "Nothing to set up on this machine."
        };
        return SetUp { ok: true, dialog: false, message: message.into() };
    }
    if !caller.may_set_up() {
        return SetUp { ok: false, dialog: false, message: caller.refusal().into() };
    }
    match os.install_wake_helper() {
        Ok(()) => SetUp { ok: true, dialog: true, message: "Power helper set up.".into() },
        Err(e) => SetUp { ok: false, dialog: true, message: format!("Lid closed will still sleep. {e}") },
    }
}

// ── Daemon wiring ──────────────────────────────────────────────────────

static KEEP_AWAKE: parking_lot::RwLock<Option<Arc<KeepAwake>>> = parking_lot::RwLock::new(None);

/// Drop the Keep awake controller so the next use builds it on the
/// current power layer ([`super::install_os`], tests).
pub fn reset_for_tests() {
    *KEEP_AWAKE.write() = None;
}

pub fn keep_awake() -> Arc<KeepAwake> {
    if let Some(k) = KEEP_AWAKE.read().as_ref() {
        return Arc::clone(k);
    }
    let mut w = KEEP_AWAKE.write();
    Arc::clone(w.get_or_insert_with(|| Arc::new(KeepAwake::new(super::power()))))
}

fn saved() -> (KeepAwakeSettings, bool) {
    let s = k2_core::app_settings::load();
    (s.keep_awake, s.wake_scheduler.wake_on_battery)
}

/// Read the saved mode and apply it. Blocking.
pub fn reconcile_now() -> Shown {
    let (settings, battery) = saved();
    keep_awake().reconcile(&settings, battery, Instant::now())
}

async fn reconcile_async(context: &'static str) {
    if let Err(e) = tokio::task::spawn_blocking(reconcile_now).await {
        k2_core::log_debug!("[keep-awake] reconcile join error ({context}): {e}");
    }
}

/// Start the loop: follow session activity, re-check every
/// [`RECONCILE_INTERVAL`] (grace, power source, settings).
pub fn spawn() {
    tokio::spawn(async move {
        let os = super::power().os();
        let _ = tokio::task::spawn_blocking(move || {
            os.clear_stale_lid_hold();
            resolve_lid_switch_once(os.as_ref());
        })
        .await;
        reconcile_async("boot").await;
        let mut events = crate::session_events::subscribe();
        let mut tick = tokio::time::interval(RECONCILE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            use crate::session_events::SessionEvent;
            use tokio::sync::broadcast::error::RecvError;
            tokio::select! {
                ev = events.recv() => match ev {
                    Ok(SessionEvent::SessionActivityChanged { workspace_path, agent_name, status, .. }) => {
                        let key = WorkTracker::key(&workspace_path, &agent_name);
                        if keep_awake().note_activity(&key, &status, Instant::now()) {
                            reconcile_async("agent working").await;
                        }
                    }
                    Ok(SessionEvent::SessionRemoved { workspace_path, agent_name, .. }) => {
                        keep_awake().note_removed(&WorkTracker::key(&workspace_path, &agent_name), Instant::now());
                    }
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                },
                _ = tick.tick() => reconcile_async("tick").await,
            }
        }
    });
}

/// Body for `POST /cli/power/keep-awake`.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetBody {
    /// `off` | `working` | `always`. Omitted = unchanged.
    mode: Option<String>,
    /// 0.43.0/0.43.1 clients' "Allow lid closed" (P19): turns the lid
    /// switch on and runs [`set_up`] (a local Admin or Owner only).
    #[serde(default)]
    approve_lid: bool,
    /// "Also on battery" (shared with wake, D12). Omitted = unchanged.
    on_battery: Option<bool>,
    /// "Also with the lid closed". Omitted = unchanged. Never prompts.
    lid_closed: Option<bool>,
}

/// `GET /cli/power/status` — what Keep awake really holds, plus the S2
/// wake/awake objects.
pub fn handle_status(caller: HelperCaller) -> crate::cli_response::CliResponse {
    reconcile_now();
    let (settings, battery) = saved();
    let mut v = keep_awake().status_json(&settings, battery, caller);
    let power = super::power().status_json();
    v["awake"] = power["awake"].clone();
    v["wake"] = power["wake"].clone();
    crate::cli_response::CliResponse::ok_json(serde_json::json!({ "keepAwake": v }).to_string())
}

/// `POST /cli/power/keep-awake` — set the mode, "Also on battery" and
/// "Also with the lid closed". None of them installs anything or shows a
/// dialog: without the helper the status says lid closed will still
/// sleep and offers **Set up**. Only an old client's `approveLid` reaches
/// [`set_up`], which itself refuses a Member or a remote client.
pub fn handle_set(body: &[u8], caller: HelperCaller) -> crate::cli_response::CliResponse {
    use crate::cli_response::CliResponse;
    let parsed: SetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };
    let current = k2_core::app_settings::load().keep_awake;
    let mode = match &parsed.mode {
        Some(m) => match KeepAwakeMode::from_wire(m) {
            Some(mode) => mode,
            None => return CliResponse::bad_request(format!("mode must be one of off|working|always (got {m:?})")),
        },
        None => current.mode(),
    };
    let mut partial = serde_json::json!({ "keepAwake": { "mode": mode.as_wire() } });
    match (parsed.lid_closed, parsed.approve_lid) {
        (_, true) => partial["keepAwake"]["lidClosed"] = serde_json::Value::Bool(true),
        (Some(b), false) => partial["keepAwake"]["lidClosed"] = serde_json::Value::Bool(b),
        (None, false) => {}
    }
    if let Some(b) = parsed.on_battery {
        partial["wakeScheduler"] = serde_json::json!({ "wakeOnBattery": b });
    }
    if let Err(e) = k2_core::app_settings::update(partial) {
        return CliResponse::bad_request(e);
    }
    let setup = if parsed.approve_lid { Some(set_up(super::power().os().as_ref(), caller)) } else { None };
    let shown = reconcile_now();
    // Wake re-plans too: "Also on battery" is shared, and a new helper
    // lets wake schedule.
    if parsed.on_battery.is_some() || setup.as_ref().is_some_and(|s| s.dialog && s.ok) {
        super::replan_wake_now();
    }
    let (settings, battery) = saved();
    let mut v = keep_awake().status_json(&settings, battery, caller);
    v["message"] = serde_json::Value::String(match setup {
        Some(s) => s.message,
        None => shown.detail,
    });
    CliResponse::ok_json(serde_json::json!({ "success": true, "keepAwake": v }).to_string())
}

/// Body for `POST /cli/power/helper`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperBody {
    /// `setup` in S1. `open-settings` and `remove` come with the
    /// SMAppService helper (S3).
    action: String,
}

/// `POST /cli/power/helper {action:"setup"}` — **Set up** for lid closed
/// (and wake). Admin floor in `route_policy.rs`; here it also refuses a
/// caller that is not at the host itself, because the dialog shows on
/// the host's screen. 403 before the body is looked at.
pub fn handle_helper(body: &[u8], caller: HelperCaller) -> crate::cli_response::CliResponse {
    use crate::cli_response::CliResponse;
    let forbidden = |error: &str, message: &str| CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({ "error": error, "message": message }).to_string(),
    };
    if !caller.admin {
        return forbidden("role_required", caller.refusal());
    }
    if !caller.local {
        return forbidden("host_only", caller.refusal());
    }
    let parsed: HelperBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };
    if parsed.action != "setup" {
        return CliResponse::bad_request(format!("action must be \"setup\" (got {:?})", parsed.action));
    }
    let r = set_up(super::power().os().as_ref(), caller);
    reconcile_now();
    if r.dialog && r.ok {
        super::replan_wake_now();
    }
    let (settings, battery) = saved();
    let mut v = keep_awake().status_json(&settings, battery, caller);
    v["message"] = serde_json::Value::String(r.message.clone());
    CliResponse::ok_json(
        serde_json::json!({ "success": r.ok, "message": r.message, "keepAwake": v }).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::super::fake::FakePowerOs;
    use super::*;

    const AC: PowerSource = PowerSource { on_ac: Some(true), battery_percent: Some(90) };

    fn batt(p: u8) -> PowerSource {
        PowerSource { on_ac: Some(false), battery_percent: Some(p) }
    }

    /// A mode with "Also with the lid closed" on.
    fn settings(mode: &str) -> KeepAwakeSettings {
        settings_lid(mode, Some(true))
    }

    fn settings_lid(mode: &str, lid_closed: Option<bool>) -> KeepAwakeSettings {
        KeepAwakeSettings { mode: mode.into(), lid_dialog_declined: false, lid_closed }
    }

    const LOCAL_ADMIN: HelperCaller = HelperCaller { local: true, admin: true };
    const NOT_ALLOWED: [HelperCaller; 3] = [
        HelperCaller { local: true, admin: false },
        HelperCaller { local: false, admin: true },
        HelperCaller { local: false, admin: false },
    ];

    fn mac_ready() -> LidFacts {
        LidFacts { access: LidAccess::Ready, ac_only_unless_allowed: true }
    }

    fn rig(fake: FakePowerOs) -> (Arc<FakePowerOs>, KeepAwake) {
        let fake = Arc::new(fake);
        let power = Arc::new(Power::new(fake.clone()));
        (fake, KeepAwake::new(power))
    }

    /// Mode transitions: Off → Always → Working (idle) → Always → Off,
    /// each with what the fake OS really holds.
    #[test]
    fn mode_transitions_take_and_drop_the_holds() {
        let (fake, ka) = rig(FakePowerOs::mac_with_helper());
        let t0 = Instant::now();

        let s = ka.reconcile(&settings("off"), false, t0);
        assert_eq!((s.state, s.held), ("off", false));
        assert_eq!((fake.holds_taken(), fake.lid_taken()), (0, 0));
        assert_eq!(fake.power_reads(), 0, "Off reads nothing from the OS");

        let s = ka.reconcile(&settings("always"), false, t0);
        assert_eq!(s.state, "lid_closed_ok", "{s:?}");
        assert_eq!(s.label, "Awake, lid closed OK (on power)");
        assert_eq!((fake.holds_taken(), fake.lid_taken()), (1, 1));

        // Re-applying the same mode takes nothing new.
        ka.reconcile(&settings("always"), false, t0);
        assert_eq!((fake.holds_taken(), fake.lid_taken()), (1, 1));

        let s = ka.reconcile(&settings("working"), false, t0);
        assert_eq!((s.state, s.held), ("waiting", false));
        assert_eq!((fake.holds_released(), fake.lid_released()), (1, 1));

        let s = ka.reconcile(&settings("always"), false, t0);
        assert_eq!(s.state, "lid_closed_ok");
        assert_eq!((fake.holds_taken(), fake.lid_taken()), (2, 2));

        let s = ka.reconcile(&settings("off"), false, t0);
        assert_eq!((s.state, s.held, s.lid_held), ("off", false, false));
        assert_eq!((fake.holds_released(), fake.lid_released()), (2, 2));
    }

    /// "While agents are working" holds on `working`, keeps holding
    /// through the grace, then releases.
    #[test]
    fn while_working_follows_the_activity_signal_with_a_grace() {
        let (fake, ka) = rig(FakePowerOs::mac_with_helper());
        let t0 = Instant::now();
        let s = settings("working");

        assert_eq!(ka.reconcile(&s, false, t0).state, "waiting");
        assert_eq!(fake.holds_taken(), 0);

        let a = WorkTracker::key("/w/sew", "sew-chat");
        let b = WorkTracker::key("/w/k2", "k2-chat");
        assert!(ka.note_activity(&a, "working", t0), "first working session starts a hold");
        assert!(!ka.note_activity(&b, "working", t0), "already holding");
        assert_eq!(ka.reconcile(&s, false, t0).state, "lid_closed_ok");
        assert_eq!(fake.holds_taken(), 1);

        // One goes idle: the other still works.
        assert!(!ka.note_activity(&a, "idle", t0 + Duration::from_secs(5)));
        assert_eq!(ka.reconcile(&s, false, t0 + Duration::from_secs(5)).state, "lid_closed_ok");
        // The last one asks for permission (not working): grace starts.
        ka.note_activity(&b, "permission", t0 + Duration::from_secs(10));
        let in_grace = t0 + Duration::from_secs(10) + WORKING_GRACE - Duration::from_secs(1);
        assert_eq!(ka.reconcile(&s, false, in_grace).state, "lid_closed_ok", "still in the grace");
        assert_eq!(fake.holds_released(), 0);
        let after = t0 + Duration::from_secs(10) + WORKING_GRACE;
        let shown = ka.reconcile(&s, false, after);
        assert_eq!((shown.state, shown.held), ("waiting", false));
        assert_eq!((fake.holds_released(), fake.lid_released()), (1, 1));

        // Working again inside a fresh run, then the session is removed.
        let t1 = after + Duration::from_secs(1);
        assert!(ka.note_activity(&a, "working", t1));
        assert_eq!(ka.reconcile(&s, false, t1).state, "lid_closed_ok");
        ka.note_removed(&a, t1);
        assert_eq!(ka.reconcile(&s, false, t1 + WORKING_GRACE).state, "waiting");
    }

    /// A lost idle event cannot hold the machine forever.
    #[test]
    fn a_session_stuck_working_stops_counting() {
        let mut t = WorkTracker::default();
        let t0 = Instant::now();
        t.on_activity("x|y", "working", t0);
        assert!(t.busy(t0 + WORKING_STALE - Duration::from_secs(1)));
        assert!(!t.busy(t0 + WORKING_STALE), "stale working sessions drop with no grace");
        assert_eq!(t.working_count(), 0);
    }

    /// D12 — AC only by default; "Also on battery" allows it above 20 %;
    /// below 20 % on battery everything pauses.
    #[test]
    fn battery_floor_and_ac_only() {
        let lid = mac_ready();
        let p = plan(KeepAwakeMode::Always, false, AC, false, true, &lid);
        assert_eq!((p.hold_open, p.want_lid, p.paused_battery), (true, true, false));

        let p = plan(KeepAwakeMode::Always, false, batt(60), false, true, &lid);
        assert_eq!((p.hold_open, p.want_lid), (true, false), "battery without the checkbox: lid open only");
        let why = p.lid_block.clone().expect("a reason");
        assert!(why.contains("Also on battery"), "{why}");

        let p = plan(KeepAwakeMode::Always, false, batt(60), true, true, &lid);
        assert_eq!((p.hold_open, p.want_lid), (true, true), "checkbox on, above the floor");

        let p = plan(KeepAwakeMode::Always, false, batt(20), true, true, &lid);
        assert_eq!((p.hold_open, p.want_lid, p.paused_battery), (true, true, false), "20% is at the floor, not below");

        for allowed in [false, true] {
            let p = plan(KeepAwakeMode::Always, false, batt(19), allowed, true, &lid);
            assert_eq!((p.hold_open, p.want_lid, p.paused_battery), (false, false, true));
        }

        // A desktop with no battery counts as power.
        let p = plan(KeepAwakeMode::Always, false, PowerSource::default(), false, true, &lid);
        assert_eq!((p.hold_open, p.want_lid), (true, true));

        // Linux/Windows lid holds are not the Mac's battery-blind one.
        let other = LidFacts { access: LidAccess::Ready, ac_only_unless_allowed: false };
        let p = plan(KeepAwakeMode::Always, false, batt(60), false, true, &other);
        assert_eq!(p.want_lid, true);
    }

    /// The controller drops the lid hold when the Mac goes on battery,
    /// and pauses everything below 20 %.
    #[test]
    fn unplugging_releases_the_lid_hold_and_low_battery_pauses() {
        let (fake, ka) = rig(FakePowerOs::mac_with_helper());
        let t0 = Instant::now();
        let s = settings("always");
        assert_eq!(ka.reconcile(&s, false, t0).state, "lid_closed_ok");

        fake.set_source(batt(55));
        let shown = ka.reconcile(&s, false, t0);
        assert_eq!(shown.state, "lid_open_only");
        assert_eq!(shown.label, "Awake (lid open only)");
        assert!(shown.detail.starts_with("Lid closed will still sleep"), "{}", shown.detail);
        assert_eq!((fake.lid_released(), fake.holds_released()), (1, 0));

        let shown = ka.reconcile(&s, true, t0);
        assert_eq!(shown.label, "Awake, lid closed OK (on battery)");
        assert_eq!(fake.lid_taken(), 2);

        fake.set_source(batt(12));
        let shown = ka.reconcile(&s, true, t0);
        assert_eq!((shown.state, shown.label.as_str(), shown.held), ("paused", "Paused: battery below 20%", false));
        assert_eq!((fake.lid_released(), fake.holds_released()), (2, 1));
    }

    /// Power-helper S1 T1 — on a Mac without the helper, no mode, no
    /// switch position and no caller ever reaches the installer through
    /// the policy: Keep awake holds with the lid open and says lid closed
    /// is not set up.
    #[test]
    fn a_mode_change_never_installs_the_helper() {
        let (fake, ka) = rig(FakePowerOs::needs_approval(true));
        let t0 = Instant::now();
        ka.note_activity(&WorkTracker::key("/w/k2", "k2-chat"), "working", t0);
        for lid in [None, Some(false), Some(true)] {
            for mode in ["off", "working", "always", "working", "off"] {
                let s = settings_lid(mode, lid);
                let shown = ka.reconcile(&s, false, t0);
                for caller in [LOCAL_ADMIN, NOT_ALLOWED[0], NOT_ALLOWED[1], NOT_ALLOWED[2]] {
                    let _ = ka.status_json(&s, false, caller);
                }
                assert_eq!(fake.approval_prompts(), 0, "mode {mode} lid {lid:?} prompted");
                if mode != "off" {
                    assert_eq!(shown.state, "lid_open_only", "{mode} {lid:?}: {shown:?}");
                    assert_eq!(shown.label, "Awake (lid open only)");
                    let why = if lid == Some(true) { LID_NOT_SET_UP } else { LID_SWITCH_OFF };
                    assert_eq!(shown.detail, format!("Lid closed will still sleep: {why}"));
                }
            }
        }
        assert_eq!(fake.lid_taken(), 0, "no lid hold without the helper");
        assert!(fake.holds_taken() >= 1, "the lid-open hold needs no helper");
    }

    /// T2 — `plan()` without the helper on AC: lid open holds, lid closed
    /// is not wanted, and the reason names the helper.
    #[test]
    fn plan_without_the_helper_holds_lid_open_only() {
        let lid = LidFacts { access: LidAccess::NeedsApproval, ac_only_unless_allowed: true };
        let p = plan(KeepAwakeMode::Always, false, AC, false, true, &lid);
        assert_eq!((p.hold_open, p.want_lid), (true, false));
        assert_eq!(p.lid_block.as_deref(), Some(LID_NOT_SET_UP));
    }

    /// The switch: off holds lid open only on every OS where K2 holds the
    /// lid; on holds lid closed; Windows has no switch (the plan decides).
    #[test]
    fn the_lid_switch_decides_the_lid_hold() {
        let ready = mac_ready();
        let p = plan(KeepAwakeMode::Always, false, AC, false, false, &ready);
        assert_eq!((p.hold_open, p.want_lid), (true, false));
        assert_eq!(p.lid_block.as_deref(), Some(LID_SWITCH_OFF));
        let p = plan(KeepAwakeMode::Always, false, AC, false, true, &ready);
        assert_eq!((p.hold_open, p.want_lid, p.lid_block), (true, true, None));
        // Not holding at all: the switch adds nothing.
        let p = plan(KeepAwakeMode::Working, false, AC, false, true, &ready);
        assert_eq!((p.hold_open, p.want_lid, p.lid_block), (false, false, None));

        let (fake, ka) = rig(FakePowerOs::mac_with_helper());
        let t0 = Instant::now();
        let shown = ka.reconcile(&settings_lid("always", Some(false)), false, t0);
        assert_eq!((shown.state, shown.lid_held), ("lid_open_only", false));
        assert_eq!(shown.detail, format!("Lid closed will still sleep: {LID_SWITCH_OFF}"));
        assert_eq!((fake.holds_taken(), fake.lid_taken()), (1, 0));
        // Turning it on takes the lid hold; off again drops it, keeps open.
        assert_eq!(ka.reconcile(&settings_lid("always", Some(true)), false, t0).state, "lid_closed_ok");
        assert_eq!(fake.lid_taken(), 1);
        let shown = ka.reconcile(&settings_lid("always", None), false, t0);
        assert_eq!(shown.state, "lid_open_only", "unset reads as off");
        assert_eq!((fake.lid_released(), fake.holds_released()), (1, 0));

        // Windows: no switch, the power plan decides as before.
        let (_w, ka) = rig(FakePowerOs::windows_lid_sleeps());
        let shown = ka.reconcile(&settings_lid("always", None), false, t0);
        assert!(shown.detail.contains("Do nothing"), "{}", shown.detail);
        assert_eq!(ka.status_json(&settings_lid("always", None), false, LOCAL_ADMIN)["lidSwitch"], false);
    }

    /// The status offers Set up only to a local Admin or Owner, and the
    /// old clients' `canApproveLid` follows it.
    #[test]
    fn status_offers_set_up_only_to_a_local_admin() {
        let (_fake, ka) = rig(FakePowerOs::needs_approval(true));
        let s = settings_lid("always", Some(true));
        ka.reconcile(&s, false, Instant::now());
        let v = ka.status_json(&s, false, LOCAL_ADMIN);
        assert_eq!(v["lidClosed"], true);
        assert_eq!(v["lidSwitch"], true);
        assert_eq!(v["lidSetup"], "needs_setup");
        assert_eq!(v["canSetUp"], true);
        assert_eq!(v["canApproveLid"], true);
        assert_eq!(v["lidDialogDeclined"], false);
        assert!(v["lidSetupDetail"].as_str().expect("detail").contains("admin password"), "{v}");
        for caller in NOT_ALLOWED {
            let v = ka.status_json(&s, false, caller);
            assert_eq!(v["lidSetup"], "needs_setup", "{caller:?}");
            assert_eq!(v["canSetUp"], false, "{caller:?}");
            assert_eq!(v["canApproveLid"], false, "{caller:?}");
            let d = v["lidSetupDetail"].as_str().expect("detail");
            assert!(d.contains("host Mac"), "{caller:?}: {d}");
            assert!(!d.contains("password"), "{caller:?}: {d}");
        }
        // Off reads no power source, but still says whether it is set up.
        let (fake, ka) = rig(FakePowerOs::needs_approval(true));
        let off = settings_lid("off", Some(true));
        ka.reconcile(&off, false, Instant::now());
        assert_eq!(ka.status_json(&off, false, LOCAL_ADMIN)["lidSetup"], "needs_setup");
        assert_eq!(fake.power_reads(), 0);
        // With the helper in there is nothing to set up.
        let (_f, ka) = rig(FakePowerOs::mac_with_helper());
        let v = ka.status_json(&settings("working"), false, LOCAL_ADMIN);
        assert_eq!((v["lidSetup"].as_str(), v["canSetUp"].as_bool()), (Some("ready"), Some(false)));
    }

    /// Set up: the only path to the dialog. A Member or a remote client
    /// never reaches it; a local admin does, once; an installed helper
    /// needs nothing; a decline says lid closed will still sleep.
    #[test]
    fn set_up_runs_the_dialog_only_for_a_local_admin() {
        for caller in NOT_ALLOWED {
            let fake = FakePowerOs::needs_approval(true);
            let r = set_up(&fake, caller);
            assert_eq!((r.ok, r.dialog), (false, false), "{caller:?}");
            assert_eq!(fake.approval_prompts(), 0, "{caller:?} must never cause the dialog");
            assert!(r.message.contains("host Mac"), "{caller:?}: {}", r.message);
        }
        let (fake, ka) = rig(FakePowerOs::needs_approval(true));
        let r = set_up(fake.as_ref(), LOCAL_ADMIN);
        assert_eq!((r.ok, r.dialog), (true, true));
        assert_eq!(fake.approval_prompts(), 1);
        assert_eq!(ka.reconcile(&settings("always"), false, Instant::now()).state, "lid_closed_ok");
        // Now in: Set up again shows nothing.
        let r = set_up(fake.as_ref(), LOCAL_ADMIN);
        assert_eq!((r.ok, r.dialog, r.message.as_str()), (true, false, "The power helper is already set up."));
        assert_eq!(fake.approval_prompts(), 1);

        let declining = FakePowerOs::needs_approval(false);
        let r = set_up(&declining, LOCAL_ADMIN);
        assert_eq!((r.ok, r.dialog), (false, true));
        assert!(r.message.starts_with("Lid closed will still sleep."), "{}", r.message);

        let linux = FakePowerOs::linux_session();
        let r = set_up(&linux, LOCAL_ADMIN);
        assert_eq!((r.ok, r.dialog, r.message.as_str()), (true, false, "Nothing to set up on this machine."));
        assert_eq!(linux.approval_prompts(), 0);
    }

    /// The first boot after the upgrade: a Mac that approved the 0.43.0
    /// helper keeps lid closed (switch on, in any mode); a Mac without it
    /// starts off; Linux keeps it only if a mode was on; Windows has no
    /// switch. Off reads no power source.
    #[test]
    fn the_switch_starts_from_the_old_behaviour() {
        use KeepAwakeMode::{Always, Off, Working};
        let mac = FakePowerOs::mac_with_helper();
        for mode in [Off, Working, Always] {
            assert!(migrated_lid_switch(mode, &mac), "{mode:?}: an approved 0.43.0 helper keeps lid closed");
        }
        let fresh = FakePowerOs::needs_approval(true);
        for mode in [Off, Working, Always] {
            assert!(!migrated_lid_switch(mode, &fresh), "{mode:?}: no helper, the switch starts off");
        }
        assert_eq!(fresh.approval_prompts(), 0);
        let linux = FakePowerOs::linux_session();
        assert!(!migrated_lid_switch(Off, &linux));
        assert!(migrated_lid_switch(Working, &linux));
        assert!(!migrated_lid_switch(Always, &FakePowerOs::windows_lid_sleeps()));
        assert_eq!((mac.power_reads(), linux.power_reads()), (0, 0));
    }

    /// D13 / D14 — Linux headless and the Windows lid action are honest.
    #[test]
    fn linux_headless_and_windows_lid_action_are_reported() {
        let (_fake, ka) = rig(FakePowerOs::linux_no_session());
        let shown = ka.reconcile(&settings("always"), false, Instant::now());
        assert_eq!(shown.state, "limited");
        assert!(shown.label.contains("limited (no session)"), "{}", shown.label);

        let (fake, ka) = rig(FakePowerOs::linux_session());
        let shown = ka.reconcile(&settings("always"), false, Instant::now());
        assert_eq!(shown.state, "lid_closed_ok");
        assert_eq!(fake.lid_taken(), 1);

        let (fake, ka) = rig(FakePowerOs::windows_lid_sleeps());
        let shown = ka.reconcile(&settings("always"), false, Instant::now());
        assert_eq!(shown.state, "lid_open_only");
        assert!(shown.detail.contains("Do nothing"), "the steps are shown: {}", shown.detail);
        assert_eq!(fake.lid_taken(), 0, "K2 never changes the plan");
    }

    /// A refused shared assertion is an error, not a fake "Awake".
    #[test]
    fn a_refused_assertion_is_not_shown_as_awake() {
        let (_fake, ka) = rig(FakePowerOs::assertion_refused("IOPMAssertionCreateWithName returned 0xe00002c1"));
        let shown = ka.reconcile(&settings("always"), false, Instant::now());
        assert_eq!((shown.state, shown.held), ("error", false));
        assert!(shown.detail.contains("0xe00002c1"), "{}", shown.detail);
    }
}
