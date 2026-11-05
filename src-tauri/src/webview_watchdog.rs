// ── 0.39.x (Issue #6): webview liveness watchdog ─────────────────────
//
// K2 can land in a black, unresponsive window where the renderer JS
// isn't running — in TWO situations:
//   1. At LAUNCH (esp. after an auto-update over the running process):
//      WKWebView loads index.html but never executes the JS bundle.
//   2. MID-SESSION: the WKWebView content process dies and respawns
//      blank — most commonly when the laptop sleeps and wakes.
// In both, right-click → Reload fixes it, but ordinary users won't know
// to do that, so the app reads as broken.
//
// This is UPSTREAM of every renderer-side black-screen defense
// (ConnectionGate, dynamic-import, /boot-status handshake) — they all live
// in the renderer JS and cannot fire when the renderer JS itself isn't
// running. Recovery has to come from Rust.
//
// Mechanism: a renderer HEARTBEAT. `index.tsx` calls `renderer_heartbeat`
// the instant the bundle executes and then on a ~3s timer, with whether the
// page is visible. This thread tracks each window's last beat; a window
// silent past a threshold for a couple of consecutive checks is PROBED with
// an eval round-trip, and only reloaded when the probe goes unanswered.
//
// prd-zen-user-widgets-v2 UW32 / UW56: the watchdog is PER WINDOW. Each K2
// UI window (`main`, `window-*`, `focus-*`) beats with its own label, and a
// frozen window is reloaded on its own: a custom Garden widget stuck in
// `while (true) {}` freezes only the window it runs in.
//
// 0.46.9 — false reloads. z3mbp's field log had ~38,000 reload lines, almost
// all "attempt 1/3" (the reloaded page beat at once, so the renderer was
// alive) and in runs of up to 15,517 for one window. A hidden, minimised or
// occluded window has its JS timers throttled by WebKit / App Nap, so the
// 3 s beat stops; the old watchdog then reloaded that healthy window every
// ~15 s for as long as it stayed hidden (loses state, re-runs everything,
// burns CPU). Now:
//   - a natively hidden or minimised window is never reloaded (its clock
//     restarts when it is shown again);
//   - a page that reported itself hidden gets a 60 s threshold, not 9 s;
//   - nothing is reloaded until an eval probe goes unanswered: a throttled
//     but live renderer still answers an eval, a `while (true)` or a dead
//     content process can't;
//   - staleness is ignored right after the Mac sleeps (a tick gap);
//   - at most `max_reloads_per_hour` reloads per window per rolling hour.
//
// Wall-clock millis (SystemTime, not Instant) so a long sleep counts as
// elapsed time.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// A window's last sign of life: when, and whether the page said it was
/// visible (`document.visibilityState === 'visible'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Beat {
    pub at_ms: u64,
    pub visible: bool,
}

static WINDOW_BEATS: Mutex<Option<HashMap<String, Beat>>> = Mutex::new(None);
// Windows the watchdog reloaded, label -> when. The renderer takes its note
// at boot (`watchdog_take_reload_note`) so a Garden whose custom widgets
// were running can tell a watchdog reload from a normal start (UW32).
static WATCHDOG_RELOAD_NOTES: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

fn with_map<V, T>(m: &Mutex<Option<HashMap<String, V>>>, f: impl FnOnce(&mut HashMap<String, V>) -> T) -> T {
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Default::default))
}

fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn stamp_beat(label: &str, visible: bool) {
    with_map(&WINDOW_BEATS, |m| m.insert(label.to_string(), Beat { at_ms: now_unix_millis(), visible }));
}

/// Renderer liveness heartbeat — invoked from `index.tsx` the moment the
/// bundle executes, then on a ~3s timer and on every visibility change.
/// Stamps the watchdog for THIS window (Tauri injects the calling window).
/// `visible` is `document.visibilityState === 'visible'`; a caller that
/// doesn't send it counts as visible (the strict, old behaviour).
#[tauri::command]
pub fn renderer_heartbeat(window: tauri::WebviewWindow, visible: Option<bool>) {
    stamp_beat(window.label(), visible.unwrap_or(true));
}

/// When the watchdog last reloaded the calling window (unix ms), taken
/// once: the renderer asks at boot (UW32).
#[tauri::command]
pub fn watchdog_take_reload_note(window: tauri::WebviewWindow) -> Option<u64> {
    with_map(&WATCHDOG_RELOAD_NOTES, |m| m.remove(window.label()))
}

/// The eval probe. `index.tsx` sets `window.__k2RendererAlive` when the
/// bundle runs, so a page whose bundle never executed (the launch black
/// screen) answers `k2-no-app` and is still reloaded.
pub(crate) const PROBE_JS: &str =
    "(window.__k2RendererAlive === true ? 'k2-pong:' + document.visibilityState : 'k2-no-app')";

/// The probe's answer as wry hands it back (the JSON of the result; an
/// empty string when the eval failed). `Some(visible)` when the app's
/// renderer answered, `None` otherwise.
pub(crate) fn parse_probe_answer(result: &str) -> Option<bool> {
    let s = result.trim();
    let s = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s);
    s.strip_prefix("k2-pong:").map(|v| v == "visible")
}

/// The K2 UI windows the watchdog guards (`k2_app_window.rs`): `main`,
/// `window-<uuid>`, `focus-<project>`. Browser child webviews are not
/// webview windows and never beat.
pub(crate) fn is_watched_window(label: &str) -> bool {
    label == "main" || label.starts_with("window-") || label.starts_with("focus-")
}

/// The watchdog's thresholds. One set in production (`LIMITS`); tests pass
/// the same values so the walk-throughs below match the shipped timing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// A visible page silent this long is stale. It beats every ~3 s, so
    /// 9 s tolerates two missed beats of ordinary main-thread jank.
    pub stale_ms: u64,
    /// The same for a page whose last beat said it was hidden: WebKit
    /// throttles a hidden page's timers, so its beats can be far apart.
    pub hidden_stale_ms: u64,
    /// Stale ticks before the probe goes out (the probe itself is the
    /// real confirmation; this absorbs one tick of jitter).
    pub min_stale_streak: u32,
    /// How long an unanswered probe waits before a reload (one tick).
    pub probe_wait_ms: u64,
    /// Reloads per stale episode before giving up (error sheet).
    pub max_reloads: u32,
    /// Reloads per window per rolling hour, whatever the episode.
    pub max_reloads_per_hour: u32,
    /// A gap this long between ticks means the Mac slept (or the shell was
    /// suspended): every window's clock restarts.
    pub wake_gap_ms: u64,
}

pub(crate) const TICK_MS: u64 = 3_000;

pub(crate) const LIMITS: Limits = Limits {
    stale_ms: 9_000,
    hidden_stale_ms: 60_000,
    min_stale_streak: 1,
    probe_wait_ms: 2_500,
    max_reloads: 3,
    max_reloads_per_hour: 6,
    wake_gap_ms: 20_000,
};

const HOUR_MS: u64 = 3_600_000;

/// What to do with one window this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchdogAction {
    /// The renderer beat since its clock last restarted — reset, keep
    /// watching.
    Healthy,
    /// No beat since the clock restarted (launch, shown again, woke, just
    /// reloaded), but inside the threshold: wait, and keep the episode's
    /// counts (a grace is not a recovery).
    Grace,
    /// The shell says the window is hidden or minimised: its timers are
    /// throttled, so silence means nothing. Never reloaded.
    Paused,
    /// Stale but not yet confirmed, or a probe is still in flight.
    Watch,
    /// Stale and confirmed — send the eval probe.
    Probe,
    /// Stale, the probe went unanswered, budget remains — reload now.
    Reload,
    /// Stale, probe unanswered, the episode's budget is spent — give up +
    /// error sheet (once per episode).
    GiveUp,
    /// Stale, probe unanswered, but the window already had
    /// `max_reloads_per_hour` reloads this hour — don't reload (said once).
    Capped,
}

/// Everything the decision looks at for one window. Plain values, so the
/// rules are unit-tested without a webview.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WatchInput {
    /// The shell (not the page) says the window is hidden or minimised.
    pub natively_hidden: bool,
    /// The page's last beat said `document.visibilityState` was not visible.
    pub renderer_hidden: bool,
    /// Since the last beat, or since the window's clock restarted (first
    /// seen, shown again, woke from sleep, reloaded), whichever is later.
    pub silent_ms: u64,
    /// The last beat is no older than the clock's restart.
    pub beat_since_clock_restart: bool,
    pub stale_streak: u32,
    /// How long an unanswered probe has been out (None = no probe out, or
    /// it was answered).
    pub probe_pending_ms: Option<u64>,
    pub reloads_in_episode: u32,
    pub reloads_last_hour: u32,
}

/// The watchdog's rules for one window, in order:
/// 1. hidden or minimised (by the shell) → never reload;
/// 2. inside the threshold (9 s; 60 s when the page said it was hidden) →
///    healthy if it beat since its clock restarted, else grace;
/// 3. not stale for `min_stale_streak` ticks yet → watch;
/// 4. no probe out → probe; probe younger than a tick → watch;
/// 5. probe unanswered → reload, unless the episode budget (give up) or
///    the hourly cap is spent.
pub(crate) fn watchdog_decision(i: &WatchInput, l: &Limits) -> WatchdogAction {
    if i.natively_hidden {
        return WatchdogAction::Paused;
    }
    let threshold = if i.renderer_hidden { l.hidden_stale_ms } else { l.stale_ms };
    if i.silent_ms <= threshold {
        return if i.beat_since_clock_restart { WatchdogAction::Healthy } else { WatchdogAction::Grace };
    }
    if i.stale_streak < l.min_stale_streak {
        return WatchdogAction::Watch;
    }
    match i.probe_pending_ms {
        None => return WatchdogAction::Probe,
        Some(waited) if waited < l.probe_wait_ms => return WatchdogAction::Watch,
        Some(_) => {}
    }
    if i.reloads_in_episode >= l.max_reloads {
        WatchdogAction::GiveUp
    } else if i.reloads_last_hour >= l.max_reloads_per_hour {
        WatchdogAction::Capped
    } else {
        WatchdogAction::Reload
    }
}

/// One window's watchdog state.
#[derive(Debug, Default)]
pub(crate) struct WindowWatch {
    /// Staleness is measured from no earlier than this: when the watchdog
    /// first saw the window (launch grace), when it was last shown again,
    /// the last wake from sleep, or the last reload (the reloaded page gets
    /// the same grace as a launch).
    pub clock_floor_ms: u64,
    natively_hidden: bool,
    stale_streak: u32,
    probe_sent_ms: Option<u64>,
    reloads: u32,
    reload_times: VecDeque<u64>,
    gave_up: bool,
    capped: bool,
    /// The page to renavigate to when a reload can't help.
    pub app_url: Option<url::Url>,
}

impl WindowWatch {
    pub(crate) fn new(now: u64) -> Self {
        WindowWatch { clock_floor_ms: now, ..Default::default() }
    }
}

/// An open window as the thread sees it this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenWindow {
    pub label: String,
    /// `!is_visible() || is_minimized()` from the shell.
    pub natively_hidden: bool,
}

/// What to do with one window this tick.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WindowTick {
    pub label: String,
    pub action: WatchdogAction,
    /// Reloads in this stale episode before the tick (for the
    /// "recovered after N reloads" line).
    pub reloads_before: u32,
    /// The window's reloads in the last hour, this tick's included.
    pub reloads_last_hour: u32,
}

/// One watchdog tick over every open window. Pure (no webviews): a beating
/// window is never touched while a silent one is probed and then reloaded;
/// closed windows are forgotten; GiveUp and Capped are said once.
/// `woke` = the gap since the last tick says the Mac slept.
pub(crate) fn watchdog_tick(
    now: u64,
    open: &[OpenWindow],
    beats: &HashMap<String, Beat>,
    states: &mut HashMap<String, WindowWatch>,
    woke: bool,
    limits: &Limits,
) -> Vec<WindowTick> {
    states.retain(|label, _| open.iter().any(|w| w.label == *label));
    let mut out = Vec::new();
    for w in open.iter().filter(|w| is_watched_window(&w.label)) {
        let st = states.entry(w.label.clone()).or_insert_with(|| WindowWatch {
            natively_hidden: w.natively_hidden,
            ..WindowWatch::new(now)
        });
        // Just woke, or just shown again: the page's timers need a moment
        // to come back, so its silence starts counting now.
        if woke || (st.natively_hidden && !w.natively_hidden) {
            st.clock_floor_ms = now;
            st.stale_streak = 0;
            st.probe_sent_ms = None;
        }
        st.natively_hidden = w.natively_hidden;
        while st.reload_times.front().is_some_and(|t| now.saturating_sub(*t) >= HOUR_MS) {
            st.reload_times.pop_front();
        }
        let beat = beats.get(&w.label).copied();
        let last_beat = beat.map_or(0, |b| b.at_ms);
        let input = WatchInput {
            natively_hidden: w.natively_hidden,
            renderer_hidden: beat.is_some_and(|b| !b.visible),
            silent_ms: now.saturating_sub(last_beat.max(st.clock_floor_ms)),
            beat_since_clock_restart: beat.is_some_and(|b| b.at_ms >= st.clock_floor_ms),
            stale_streak: st.stale_streak,
            probe_pending_ms: st
                .probe_sent_ms
                .filter(|sent| last_beat < *sent)
                .map(|sent| now.saturating_sub(sent)),
            reloads_in_episode: st.reloads,
            reloads_last_hour: st.reload_times.len() as u32,
        };
        let reloads_before = st.reloads;
        let mut action = watchdog_decision(&input, limits);
        match action {
            WatchdogAction::Healthy => {
                st.stale_streak = 0;
                st.probe_sent_ms = None;
                st.reloads = 0;
                st.gave_up = false;
                st.capped = false;
            }
            WatchdogAction::Grace | WatchdogAction::Paused => {
                st.stale_streak = 0;
                st.probe_sent_ms = None;
            }
            WatchdogAction::Watch => st.stale_streak += 1,
            WatchdogAction::Probe => {
                st.stale_streak += 1;
                st.probe_sent_ms = Some(now);
            }
            WatchdogAction::Reload => {
                st.clock_floor_ms = now;
                st.stale_streak = 0;
                st.probe_sent_ms = None;
                st.reloads += 1;
                st.reload_times.push_back(now);
                st.capped = false;
            }
            WatchdogAction::GiveUp => {
                if st.gave_up {
                    action = WatchdogAction::Watch;
                } else {
                    st.gave_up = true;
                }
            }
            WatchdogAction::Capped => {
                if st.capped {
                    action = WatchdogAction::Watch;
                } else {
                    st.capped = true;
                }
            }
        }
        out.push(WindowTick {
            label: w.label.clone(),
            action,
            reloads_before,
            reloads_last_hour: st.reload_times.len() as u32,
        });
    }
    out
}

/// Append one line to `~/.k2/webview-watchdog.log` — the field log we ask
/// users for when the window comes up black. Every reload, recovery, cap
/// and give-up writes here with a UTC timestamp, so the reloads per day can
/// be counted. Best-effort: never fails the watchdog.
fn watchdog_field_log(line: &str) {
    if let Ok(home) = std::env::var("HOME") {
        let log_path = std::path::Path::new(&home).join(".k2").join("webview-watchdog.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
            use std::io::Write;
            let _ = writeln!(f, "{} {line}", k2_client::log_timestamp());
        }
    }
}

/// Start the watchdog thread (PER WINDOW since Zen v2 UW32, UW56). It
/// recovers the launch black-screen, mid-session content-process death and
/// a window frozen by a custom Garden widget, then shows a native error
/// sheet if reloads don't bring it back.
pub(crate) fn spawn(app: &tauri::App) {
    use tauri::Manager;
    let handle = app.handle().clone();
    let dev_url = app.config().build.dev_url.clone();
    let is_dev = tauri::is_dev();
    let mut states: HashMap<String, WindowWatch> = Default::default();
    // Capture `main`'s app URL NOW, before anything can break — the
    // hard-renavigate fallback needs a known-good destination even when the
    // webview's own URL has gone blank. Other windows are captured when
    // first seen healthy. Loopback hosts are rejected unless they are the
    // exact configured `devUrl` in `tauri::is_dev()`.
    if let Some(win) = app.get_webview_window("main") {
        let mut st = WindowWatch::new(now_unix_millis());
        st.app_url = crate::k2_app_window::watchdog_capture_app_url(win.url().ok().as_ref(), dev_url.as_ref(), is_dev);
        states.insert("main".to_string(), st);
    }
    std::thread::spawn(move || {
        let limits = LIMITS;
        let max_reloads = limits.max_reloads;
        let mut last_tick: Option<u64> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(TICK_MS));
            let windows: Vec<(String, tauri::WebviewWindow)> = handle
                .webview_windows()
                .into_iter()
                .filter(|(label, _)| is_watched_window(label))
                .collect();
            let open: Vec<OpenWindow> = windows
                .iter()
                .map(|(label, win)| OpenWindow {
                    label: label.clone(),
                    // An error reads as visible: watch it (the old rule).
                    natively_hidden: !win.is_visible().unwrap_or(true) || win.is_minimized().unwrap_or(false),
                })
                .collect();
            let beats = with_map(&WINDOW_BEATS, |m| {
                m.retain(|label, _| open.iter().any(|w| w.label == *label));
                m.clone()
            });
            let now = now_unix_millis();
            let woke = last_tick.is_some_and(|t| now.saturating_sub(t) > limits.wake_gap_ms);
            last_tick = Some(now);
            if woke {
                log_debug!("[webview-watchdog] tick gap after sleep: restarting every window's clock");
            }
            let plan = watchdog_tick(now, &open, &beats, &mut states, woke, &limits);
            for tick in plan {
                let Some((_, win)) = windows.iter().find(|(l, _)| *l == tick.label) else {
                    continue;
                };
                let label = tick.label.as_str();
                let reloads = tick.reloads_before + u32::from(tick.action == WatchdogAction::Reload);
                match tick.action {
                    WatchdogAction::Healthy => {
                        if tick.reloads_before > 0 {
                            log_debug!(
                                "[webview-watchdog] {label}: renderer recovered after {} reload(s)",
                                tick.reloads_before
                            );
                            watchdog_field_log(&format!(
                                "{label}: renderer recovered after {} reload(s)",
                                tick.reloads_before
                            ));
                        }
                        if let Some(st) = states.get_mut(label) {
                            if st.app_url.is_none() {
                                st.app_url = crate::k2_app_window::watchdog_capture_app_url(
                                    win.url().ok().as_ref(),
                                    dev_url.as_ref(),
                                    is_dev,
                                );
                            }
                        }
                    }
                    WatchdogAction::Grace | WatchdogAction::Paused | WatchdogAction::Watch => {}
                    WatchdogAction::Probe => {
                        // A throttled but live renderer answers an eval; a
                        // `while (true)` or a dead content process can't.
                        let probe_label = label.to_string();
                        if let Err(e) = win.eval_with_callback(PROBE_JS, move |result| {
                            if let Some(visible) = parse_probe_answer(&result) {
                                stamp_beat(&probe_label, visible);
                            }
                        }) {
                            log_debug!("[webview-watchdog] {label}: probe eval failed: {e}");
                        }
                    }
                    WatchdogAction::Reload => {
                        log_debug!(
                            "[webview-watchdog] {label}: renderer silent and probe unanswered — recovering webview \
                             (attempt {reloads}/{max_reloads})"
                        );
                        with_map(&WATCHDOG_RELOAD_NOTES, |m| m.insert(label.to_string(), now));
                        let app_url = states.get(label).and_then(|st| st.app_url.clone()).or_else(|| {
                            crate::k2_app_window::watchdog_capture_app_url(None, dev_url.as_ref(), is_dev)
                        });
                        // Recover natively: WKWebView's reload works with a
                        // dead JS context and respawns the content process
                        // (`eval("location.reload()")` would need the very JS
                        // context it's trying to rescue). Native reload still
                        // needs a COMMITTED page, so when the current URL is
                        // blank / off-app, or this is the last budgeted
                        // attempt, renavigate to the window's app URL.
                        let current_off_app = match win.url() {
                            Err(_) => true,
                            Ok(u) if u.as_str().is_empty() || u.as_str() == "about:blank" => true,
                            Ok(u) => match app_url.as_ref() {
                                Some(app) => u != *app,
                                None => false,
                            },
                        };
                        let last_attempt = reloads >= max_reloads;
                        let renav_target = if last_attempt || current_off_app { app_url.clone() } else { None };
                        let hourly = format!("{}/{} this hour", tick.reloads_last_hour, limits.max_reloads_per_hour);
                        if let Some(target) = renav_target {
                            let outcome = match win.navigate(target.clone()) {
                                Ok(()) => "ok".to_string(),
                                Err(e) => format!("err: {e}"),
                            };
                            log_debug!(
                                "[webview-watchdog] {label}: renavigate attempted to {target} \
                                 (attempt {reloads}/{max_reloads}, {hourly}) -> {outcome}"
                            );
                            watchdog_field_log(&format!(
                                "{label}: probe unanswered; renavigate attempted to {target} \
                                 (attempt {reloads}/{max_reloads}, {hourly}) -> {outcome}",
                            ));
                        } else {
                            let outcome = match win.reload() {
                                Ok(()) => "ok".to_string(),
                                Err(e) => format!("err: {e}"),
                            };
                            log_debug!(
                                "[webview-watchdog] {label}: native reload attempted \
                                 (attempt {reloads}/{max_reloads}, {hourly}) -> {outcome}"
                            );
                            watchdog_field_log(&format!(
                                "{label}: probe unanswered; native reload attempted \
                                 (attempt {reloads}/{max_reloads}, {hourly}) -> {outcome}",
                            ));
                        }
                    }
                    WatchdogAction::Capped => {
                        let cap = limits.max_reloads_per_hour;
                        log_debug!(
                            "[webview-watchdog] {label}: renderer silent but {cap} reloads already this hour — not reloading"
                        );
                        watchdog_field_log(&format!(
                            "{label}: probe unanswered but reload cap reached ({cap}/hour); not reloading until one ages out",
                        ));
                    }
                    WatchdogAction::GiveUp => {
                        // Once per stale episode (`watchdog_tick`).
                        // Non-blocking, so the other windows stay watched
                        // while the sheet is up.
                        log_debug!(
                            "[webview-watchdog] {label}: renderer still silent after {max_reloads} \
                             recovery attempts — giving up; surfacing error sheet"
                        );
                        watchdog_field_log(&format!(
                            "{label}: renderer JS unresponsive; {max_reloads} native recovery attempts failed",
                        ));
                        use tauri_plugin_dialog::DialogExt;
                        handle
                            .dialog()
                            .message(
                                "K2 stopped responding. Please quit and relaunch the app.\n\n\
                                 If this keeps happening, reinstalling the latest version usually fixes it.",
                            )
                            .title("K2 couldn't load")
                            .show(|_| {});
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use WatchdogAction::*;

    fn win(label: &str) -> OpenWindow {
        OpenWindow { label: label.to_string(), natively_hidden: false }
    }

    fn hidden_win(label: &str) -> OpenWindow {
        OpenWindow { label: label.to_string(), natively_hidden: true }
    }

    fn beat(at_ms: u64, visible: bool) -> Beat {
        Beat { at_ms, visible }
    }

    fn action_of(plan: &[WindowTick], label: &str) -> WatchdogAction {
        plan.iter()
            .find(|t| t.label == label)
            .map(|t| t.action)
            .unwrap_or_else(|| panic!("no tick for {label}: {plan:?}"))
    }

    /// `ticks` ticks every TICK_MS from `start` with no new beats; each
    /// tick's action for `label`.
    fn walk(
        label: &str,
        open: &[OpenWindow],
        start: u64,
        ticks: u64,
        states: &mut HashMap<String, WindowWatch>,
        beats: &HashMap<String, Beat>,
    ) -> Vec<WatchdogAction> {
        (0..ticks)
            .map(|i| action_of(&watchdog_tick(start + i * TICK_MS, open, beats, states, false, &LIMITS), label))
            .collect()
    }

    // ── the pure decision ──────────────────────────────────────────────

    fn stale_visible() -> WatchInput {
        WatchInput { silent_ms: 30_000, stale_streak: 5, beat_since_clock_restart: true, ..Default::default() }
    }

    #[test]
    fn a_natively_hidden_or_minimised_window_is_never_reloaded() {
        for probe in [None, Some(0), Some(60_000)] {
            let i = WatchInput { natively_hidden: true, probe_pending_ms: probe, silent_ms: 10 * HOUR_MS, ..stale_visible() };
            assert_eq!(watchdog_decision(&i, &LIMITS), Paused, "{i:?}");
        }
    }

    #[test]
    fn a_page_that_said_it_is_hidden_gets_the_long_threshold() {
        let i = WatchInput { renderer_hidden: true, silent_ms: 59_000, ..stale_visible() };
        assert_eq!(watchdog_decision(&i, &LIMITS), Healthy);
        let i = WatchInput { renderer_hidden: false, silent_ms: 59_000, ..stale_visible() };
        assert_eq!(watchdog_decision(&i, &LIMITS), Probe);
        // Past the long threshold it is probed, not reloaded.
        let i = WatchInput { renderer_hidden: true, silent_ms: 61_000, ..stale_visible() };
        assert_eq!(watchdog_decision(&i, &LIMITS), Probe);
    }

    #[test]
    fn visible_and_stale_is_probed_before_any_reload() {
        assert_eq!(watchdog_decision(&stale_visible(), &LIMITS), Probe);
        // A probe younger than a tick: wait for its answer.
        let i = WatchInput { probe_pending_ms: Some(1_000), ..stale_visible() };
        assert_eq!(watchdog_decision(&i, &LIMITS), Watch);
    }

    #[test]
    fn visible_and_stale_with_an_unanswered_probe_reloads() {
        let i = WatchInput { probe_pending_ms: Some(3_000), ..stale_visible() };
        assert_eq!(watchdog_decision(&i, &LIMITS), Reload);
    }

    #[test]
    fn stale_below_the_streak_only_watches() {
        for streak in 0..LIMITS.min_stale_streak {
            let i = WatchInput { stale_streak: streak, ..stale_visible() };
            assert_eq!(watchdog_decision(&i, &LIMITS), Watch, "streak {streak}");
        }
    }

    #[test]
    fn fresh_beat_is_healthy_and_grace_is_not_a_recovery() {
        let i = WatchInput {
            silent_ms: 2_000,
            stale_streak: 9,
            probe_pending_ms: Some(9_000),
            reloads_in_episode: 3,
            reloads_last_hour: 6,
            beat_since_clock_restart: true,
            ..Default::default()
        };
        assert_eq!(watchdog_decision(&i, &LIMITS), Healthy);
        let i = WatchInput { beat_since_clock_restart: false, ..i };
        assert_eq!(watchdog_decision(&i, &LIMITS), Grace);
    }

    #[test]
    fn episode_budget_gives_up_and_the_hourly_cap_holds() {
        let i = WatchInput { probe_pending_ms: Some(3_000), reloads_in_episode: 3, ..stale_visible() };
        assert_eq!(watchdog_decision(&i, &LIMITS), GiveUp);
        let i = WatchInput { probe_pending_ms: Some(3_000), reloads_last_hour: 6, ..stale_visible() };
        assert_eq!(watchdog_decision(&i, &LIMITS), Capped);
        let i = WatchInput { probe_pending_ms: Some(3_000), reloads_last_hour: 5, ..stale_visible() };
        assert_eq!(watchdog_decision(&i, &LIMITS), Reload);
    }

    #[test]
    fn probe_answers_are_parsed_strictly() {
        assert_eq!(parse_probe_answer("\"k2-pong:visible\""), Some(true));
        assert_eq!(parse_probe_answer("k2-pong:visible"), Some(true));
        assert_eq!(parse_probe_answer("\"k2-pong:hidden\""), Some(false));
        // The bundle never ran, the eval failed, or something else answered.
        assert_eq!(parse_probe_answer("\"k2-no-app\""), None);
        assert_eq!(parse_probe_answer(""), None);
        assert_eq!(parse_probe_answer("null"), None);
        assert_eq!(parse_probe_answer("\"pong\""), None);
    }

    // ── whole ticks ────────────────────────────────────────────────────

    /// The field-log pattern: a hidden window whose timers stopped used to
    /// be reloaded every ~15 s for hours. Now it is never touched.
    #[test]
    fn a_natively_hidden_window_silent_for_hours_is_never_reloaded() {
        let mut states = HashMap::new();
        let beats = HashMap::from([("window-a".to_string(), beat(0, false))]);
        let seq = walk("window-a", &[hidden_win("window-a")], 0, 2_400, &mut states, &beats);
        assert!(seq.iter().all(|a| *a == Paused), "{:?}", &seq[..10]);
    }

    /// Occluded / other Space: the shell says visible, the page says hidden
    /// and its timers are throttled. It is probed (not reloaded) and the
    /// live renderer answers, so it is never reloaded.
    #[test]
    fn an_occluded_throttled_window_that_answers_the_probe_is_never_reloaded() {
        let mut states = HashMap::new();
        let mut beats = HashMap::from([("main".to_string(), beat(0, false))]);
        let open = [win("main")];
        let mut probes = 0;
        for i in 0..1_200u64 {
            let now = i * TICK_MS;
            let a = action_of(&watchdog_tick(now, &open, &beats, &mut states, false, &LIMITS), "main");
            assert!(!matches!(a, Reload | GiveUp | Capped), "tick {i}: {a:?}");
            if a == Probe {
                probes += 1;
                // The eval answers before the next tick.
                beats.insert("main".into(), beat(now + 50, false));
            }
        }
        // One hour, a probe about every 66 s: an eval, not a reload.
        assert!((50..=60).contains(&probes), "{probes} probes in an hour");
    }

    /// A visible window whose beats are late (main-thread jank, throttling
    /// the page didn't report) but whose renderer answers is never reloaded.
    #[test]
    fn visible_and_stale_with_a_probe_that_answers_is_not_reloaded() {
        let mut states = HashMap::new();
        let mut beats = HashMap::from([("main".to_string(), beat(0, true))]);
        let open = [win("main")];
        let mut probes = 0;
        for i in 0..200u64 {
            let now = i * TICK_MS;
            let a = action_of(&watchdog_tick(now, &open, &beats, &mut states, false, &LIMITS), "main");
            assert!(!matches!(a, Reload | GiveUp | Capped), "tick {i}: {a:?}");
            if a == Probe {
                probes += 1;
                beats.insert("main".into(), beat(now + 10, true));
            }
        }
        assert!(probes > 0, "the stale window was probed");
    }

    /// A `while (true)` in a visible window: no beats, no probe answers.
    /// It must still be reloaded: fresh for 9 s, one stale tick, the
    /// probe, the reload; then the reloaded page gets a launch grace.
    #[test]
    fn a_frozen_visible_window_is_still_reloaded() {
        let mut states = HashMap::new();
        let beats = HashMap::from([("window-z".to_string(), beat(0, true))]);
        let seq = walk("window-z", &[win("window-z")], 0, 8, &mut states, &beats);
        assert_eq!(seq, vec![Healthy, Healthy, Healthy, Healthy, Watch, Probe, Reload, Grace], "{seq:?}");
    }

    /// The full frozen episode: three reloads, each after an unanswered
    /// probe, then one give-up; a beat re-arms with the counters reset.
    #[test]
    fn a_frozen_episode_reloads_three_times_then_gives_up_once() {
        let mut states = HashMap::new();
        let mut beats = HashMap::from([("main".to_string(), beat(0, true))]);
        let open = [win("main")];
        let seq = walk("main", &open, 0, 40, &mut states, &beats);
        let count = |x: WatchdogAction| seq.iter().filter(|a| **a == x).count();
        assert_eq!((count(Reload), count(GiveUp), count(Capped)), (3, 1, 0), "{seq:?}");
        // Each reload follows an unanswered probe.
        for (k, a) in seq.iter().enumerate() {
            if *a == Reload {
                assert_eq!(seq[k - 1], Probe, "{seq:?}");
            }
        }
        let now = 40 * TICK_MS;
        beats.insert("main".into(), beat(now - 1, true));
        let plan = watchdog_tick(now, &open, &beats, &mut states, false, &LIMITS);
        assert_eq!(plan[0].action, Healthy);
        assert_eq!(plan[0].reloads_before, 3, "the recovery line can say how many reloads it took");
        let st = states.get("main").expect("state");
        assert_eq!((st.stale_streak, st.reloads, st.gave_up, st.probe_sent_ms), (0, 0, false, None));
    }

    /// A window that freezes, reloads, beats, and freezes again forever:
    /// no more than `max_reloads_per_hour` reloads in any hour.
    #[test]
    fn the_hourly_reload_cap_holds() {
        let mut states = HashMap::new();
        let mut beats = HashMap::from([("main".to_string(), beat(0, true))]);
        let open = [win("main")];
        let mut reload_times = Vec::new();
        let mut capped_said = 0;
        for i in 0..(3 * HOUR_MS / TICK_MS) {
            let now = i * TICK_MS;
            match action_of(&watchdog_tick(now, &open, &beats, &mut states, false, &LIMITS), "main") {
                Reload => {
                    reload_times.push(now);
                    // The reloaded page beats once, then freezes again.
                    beats.insert("main".into(), beat(now + 100, true));
                }
                Capped => capped_said += 1,
                GiveUp => panic!("each reload recovered, so no episode runs out"),
                _ => {}
            }
        }
        for (k, t) in reload_times.iter().enumerate() {
            let in_hour = reload_times[k..].iter().take_while(|u| **u - t < HOUR_MS).count();
            assert!(in_hour <= LIMITS.max_reloads_per_hour as usize, "{in_hour} reloads in the hour from {t}");
        }
        assert!(reload_times.len() >= 12, "it keeps recovering as old reloads age out: {reload_times:?}");
        assert!(capped_said >= 1, "the cap is logged");
    }

    /// Just after the Mac wakes, every beat is old: nothing is probed or
    /// reloaded, and the clock restarts from the wake.
    #[test]
    fn just_after_wake_nothing_is_reloaded() {
        let mut states = HashMap::new();
        let beats = HashMap::from([("main".to_string(), beat(1_000, true)), ("window-b".to_string(), beat(1_000, true))]);
        let open = [win("main"), win("window-b")];
        assert!(watchdog_tick(1_000, &open, &beats, &mut states, false, &LIMITS).iter().all(|t| t.action == Healthy));
        // Eight hours later, the first tick after wake.
        let wake = 1_000 + 8 * HOUR_MS;
        for i in 0..=3 {
            let plan = watchdog_tick(wake + i * TICK_MS, &open, &beats, &mut states, i == 0, &LIMITS);
            assert!(plan.iter().all(|t| t.action == Grace), "tick {i}: {plan:?}");
        }
        // Without the wake the same tick would already be stale.
        let mut cold = HashMap::new();
        watchdog_tick(1_000, &open, &beats, &mut cold, false, &LIMITS);
        let plan = watchdog_tick(wake, &open, &beats, &mut cold, false, &LIMITS);
        assert!(plan.iter().all(|t| t.action == Watch), "{plan:?}");
        // A renderer that stays dead after the wake is still recovered.
        let seq = walk("main", &open, wake + 4 * TICK_MS, 3, &mut states, &beats);
        assert_eq!(seq, vec![Watch, Probe, Reload], "{seq:?}");
    }

    /// A window shown again after hours hidden gets a fresh clock; if its
    /// renderer is really dead it is then probed and reloaded.
    #[test]
    fn a_window_shown_again_gets_a_fresh_clock_then_is_watched() {
        let mut states = HashMap::new();
        let beats = HashMap::from([("window-a".to_string(), beat(0, true))]);
        let hidden = walk("window-a", &[hidden_win("window-a")], 0, 100, &mut states, &beats);
        assert!(hidden.iter().all(|a| *a == Paused), "{hidden:?}");
        let seq = walk("window-a", &[win("window-a")], 100 * TICK_MS, 7, &mut states, &beats);
        assert_eq!(seq, vec![Grace, Grace, Grace, Grace, Watch, Probe, Reload], "{seq:?}");
    }

    /// TUW4.5: window A beating and window B silent → only B is acted on.
    #[test]
    fn per_window_a_beating_b_silent_acts_only_on_b() {
        let mut states = HashMap::new();
        let mut beats = HashMap::from([("main".to_string(), beat(0, true)), ("window-b".to_string(), beat(0, true))]);
        let open = [win("main"), win("window-b")];
        let mut b = Vec::new();
        for i in 0..7u64 {
            let now = i * TICK_MS;
            beats.insert("main".into(), beat(now, true));
            let plan = watchdog_tick(now, &open, &beats, &mut states, false, &LIMITS);
            assert_eq!(action_of(&plan, "main"), Healthy, "tick {i}");
            b.push(action_of(&plan, "window-b"));
        }
        assert_eq!(b, vec![Healthy, Healthy, Healthy, Healthy, Watch, Probe, Reload], "{b:?}");
    }

    #[test]
    fn a_new_window_gets_the_launch_grace_and_closed_windows_are_forgotten() {
        let mut states = HashMap::new();
        let beats = HashMap::new();
        let open = [win("focus-p1")];
        assert_eq!(watchdog_tick(100_000, &open, &beats, &mut states, false, &LIMITS)[0].action, Grace);
        assert_eq!(watchdog_tick(109_000, &open, &beats, &mut states, false, &LIMITS)[0].action, Grace);
        assert_eq!(watchdog_tick(109_001, &open, &beats, &mut states, false, &LIMITS)[0].action, Watch);
        assert!(states.contains_key("focus-p1"));
        assert!(watchdog_tick(110_000, &[], &beats, &mut states, false, &LIMITS).is_empty());
        assert!(states.is_empty(), "a closed window's state is dropped");
    }

    #[test]
    fn only_k2_ui_windows_are_watched() {
        for l in ["main", "window-1b2c", "focus-proj"] {
            assert!(is_watched_window(l), "{l}");
        }
        for l in ["browser-1", "Main", "zen", ""] {
            assert!(!is_watched_window(l), "{l}");
        }
        let mut states = HashMap::new();
        let plan = watchdog_tick(1, &[win("browser-1"), win("main")], &HashMap::new(), &mut states, false, &LIMITS);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].label, "main");
    }
}

