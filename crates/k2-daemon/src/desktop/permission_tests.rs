//! The Mac permission flow on a manual clock and a fake K2 Screen: no
//! macOS prompt, no thread, no sleep.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_deskd_proto::msg::Tcc;

use super::*;

fn tcc(screen: bool, accessibility: bool) -> Tcc {
    Tcc { required: true, screen, accessibility }
}

const NONE: Tcc = Tcc { required: true, screen: false, accessibility: false };

/// K2 Screen and System Settings, scripted.
struct Fake {
    /// What K2 Screen answers (request and check alike).
    now: Mutex<Tcc>,
    /// K2 Screen isn't up yet: request and check fail.
    down: Mutex<bool>,
    requests: AtomicU32,
    checks: AtomicU32,
    restarts: AtomicU32,
    panes: Mutex<Vec<Pane>>,
    asked: Mutex<Asked>,
    saves: AtomicU32,
}

impl Fake {
    fn new(asked: Asked) -> Arc<Fake> {
        Arc::new(Fake {
            now: Mutex::new(NONE),
            down: Mutex::new(false),
            requests: AtomicU32::new(0),
            checks: AtomicU32::new(0),
            restarts: AtomicU32::new(0),
            panes: Mutex::new(Vec::new()),
            asked: Mutex::new(asked),
            saves: AtomicU32::new(0),
        })
    }
    fn set(&self, t: Tcc) {
        *self.now.lock().unwrap() = t;
    }
    fn panes(&self) -> Vec<Pane> {
        self.panes.lock().unwrap().clone()
    }
}

impl Host for Fake {
    fn request(&self) -> Result<Tcc, String> {
        if *self.down.lock().unwrap() {
            return Err("deskd_not_ready: Starting K2 Screen…".into());
        }
        self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(*self.now.lock().unwrap())
    }
    fn check(&self) -> Result<Tcc, String> {
        if *self.down.lock().unwrap() {
            return Err("deskd_not_ready".into());
        }
        self.checks.fetch_add(1, Ordering::SeqCst);
        Ok(*self.now.lock().unwrap())
    }
    fn restart_helper(&self) -> Result<(), String> {
        self.restarts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn open_pane(&self, pane: Pane) -> Result<(), String> {
        self.panes.lock().unwrap().push(pane);
        Ok(())
    }
    fn load_asked(&self) -> Asked {
        *self.asked.lock().unwrap()
    }
    fn save_asked(&self, a: Asked) -> Result<(), String> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        *self.asked.lock().unwrap() = a;
        Ok(())
    }
}

struct Rig {
    fake: Arc<Fake>,
    w: Arc<Watcher>,
    changes: Arc<AtomicU32>,
}

fn rig(asked: Asked) -> Rig {
    let fake = Fake::new(asked);
    let changes = Arc::new(AtomicU32::new(0));
    let c = changes.clone();
    let w = Watcher::manual(fake.clone(), Box::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
    }));
    Rig { fake, w, changes }
}

fn view(w: &Watcher, now: Instant) -> Permission {
    w.flow.lock().unwrap().view(now, None).expect("a Mac with the switch on has steps")
}

#[test]
fn off_then_asking_then_screen_then_accessibility_then_ready() {
    let r = rig(Asked::default());
    let t0 = Instant::now();
    assert_eq!(r.w.flow.lock().unwrap().step(), Step::Off);
    assert!(r.w.view(Some(NONE)).is_none(), "the switch is off: no steps");

    // The toggle: asking until K2 Screen fires the prompts.
    r.w.flow.lock().unwrap().turned_on(t0);
    let v = view(&r.w, t0);
    assert_eq!((v.step, v.line, v.checking), (Step::Asking, LINE_ASKING, true));

    // First round: K2 Screen fires macOS's own prompts. First time: no
    // window of K2's own.
    assert!(r.w.round(t0));
    assert_eq!(r.fake.requests.load(Ordering::SeqCst), 1);
    assert!(r.fake.panes().is_empty(), "macOS shows its prompts the first time");
    let v = view(&r.w, t0);
    assert_eq!((v.step, v.screen, v.accessibility, v.line, v.opened), (Step::WaitingScreen, "waiting", "waiting", LINE_PROMPTED, None));
    assert_eq!(*r.fake.asked.lock().unwrap(), Asked { screen: true, accessibility: true }, "remembered: macOS won't prompt twice");

    // Nobody clicked yet: a check, no change.
    let t1 = t0 + POLL;
    assert!(r.w.round(t1));
    assert_eq!(r.fake.checks.load(Ordering::SeqCst), 1);
    assert_eq!(r.fake.restarts.load(Ordering::SeqCst), 0);

    // Screen Recording allowed: K2 Screen restarts once, and System
    // Settings opens at Accessibility next.
    r.fake.set(tcc(true, false));
    let t2 = t1 + POLL;
    assert!(r.w.round(t2));
    assert_eq!(r.fake.restarts.load(Ordering::SeqCst), 1);
    assert_eq!(r.fake.panes(), vec![Pane::Accessibility]);
    let v = view(&r.w, t2);
    assert_eq!((v.step, v.screen, v.accessibility, v.line, v.opened), (Step::WaitingAccessibility, "allowed", "waiting", LINE_OPENED, Some(Pane::Accessibility)));

    // Accessibility allowed: ready, polling stops.
    r.fake.set(tcc(true, true));
    let t3 = t2 + POLL;
    assert!(!r.w.round(t3), "ready: no more rounds");
    let v = view(&r.w, t3);
    assert_eq!((v.step, v.screen, v.accessibility, v.line, v.opened, v.checking), (Step::Ready, "allowed", "allowed", LINE_READY, None, false));
    assert!(r.changes.load(Ordering::SeqCst) >= 3, "each change told the views");
    let checks = r.fake.checks.load(Ordering::SeqCst);
    assert!(!r.w.round(t3 + POLL));
    assert_eq!(r.fake.checks.load(Ordering::SeqCst), checks, "nothing to check once ready");
}

#[test]
fn prompts_dismissed_before_open_the_pane_with_the_plain_line() {
    let r = rig(Asked { screen: true, accessibility: true });
    let t0 = Instant::now();
    r.w.flow.lock().unwrap().turned_on(t0);
    assert!(r.w.round(t0));
    assert_eq!(r.fake.requests.load(Ordering::SeqCst), 1, "the prompts are still fired (macOS lists K2 Screen)");
    assert_eq!(r.fake.panes(), vec![Pane::Screen], "macOS won't prompt again: K2 opens the first missing pane");
    let v = view(&r.w, t0);
    assert_eq!((v.step, v.line, v.opened), (Step::WaitingScreen, LINE_OPENED, Some(Pane::Screen)));
    assert_eq!(v.line, "Allow K2 Screen in the window that just opened.");
    assert_eq!(r.fake.saves.load(Ordering::SeqCst), 0, "nothing new to remember");
}

#[test]
fn accessibility_already_asked_and_screen_never_opens_no_window() {
    // Screen Recording never asked: macOS prompts for it; Accessibility's
    // pane waits until Screen Recording is through.
    let r = rig(Asked { screen: false, accessibility: true });
    let t0 = Instant::now();
    r.w.flow.lock().unwrap().turned_on(t0);
    r.w.round(t0);
    assert!(r.fake.panes().is_empty(), "{:?}", r.fake.panes());
    assert_eq!(view(&r.w, t0).line, LINE_PROMPTED);
}

#[test]
fn a_grant_flip_restarts_k2_screen_exactly_once() {
    let r = rig(Asked::default());
    let t0 = Instant::now();
    r.fake.set(tcc(false, true));
    r.w.flow.lock().unwrap().turned_on(t0);
    r.w.round(t0);
    r.fake.set(tcc(true, true));
    let mut t = t0;
    for _ in 0..5 {
        t += POLL;
        r.w.round(t);
    }
    assert_eq!(r.fake.restarts.load(Ordering::SeqCst), 1);
    // The restarted K2 Screen's hello says the same: no second restart.
    r.w.saw(tcc(true, true));
    assert_eq!(r.fake.restarts.load(Ordering::SeqCst), 1);
    assert_eq!(view(&r.w, t).step, Step::Ready);
}

#[test]
fn k2_screens_own_change_event_also_restarts_it_once() {
    // A newer macOS may let K2 Screen's own preflight see the grant (its
    // tcc_changed) before the daemon's check: still restarted, once.
    let r = rig(Asked::default());
    let t0 = Instant::now();
    r.w.flow.lock().unwrap().turned_on(t0);
    r.w.round(t0);
    r.w.saw(tcc(true, false));
    assert_eq!(r.fake.restarts.load(Ordering::SeqCst), 1);
    r.fake.set(tcc(true, false));
    r.w.round(t0 + POLL);
    assert_eq!(r.fake.restarts.load(Ordering::SeqCst), 1, "the check agrees: no second restart");
}

#[test]
fn a_first_hello_with_grants_restarts_nothing() {
    let r = rig(Asked::default());
    let t0 = Instant::now();
    r.w.flow.lock().unwrap().turned_on(t0);
    r.w.saw(tcc(true, true));
    r.fake.set(tcc(true, true));
    assert!(!r.w.round(t0), "nothing missing: no more rounds");
    assert_eq!(r.fake.requests.load(Ordering::SeqCst), 1, "the toggle still asks (K2 Screen has nothing to prompt for)");
    assert_eq!(r.fake.restarts.load(Ordering::SeqCst), 0);
    assert!(r.fake.panes().is_empty());
    assert_eq!(view(&r.w, t0).step, Step::Ready);
}

#[test]
fn polling_stops_after_its_limit_and_check_again_resumes_it() {
    let r = rig(Asked::default());
    let t0 = Instant::now();
    r.w.flow.lock().unwrap().turned_on(t0);
    r.w.round(t0);
    let mut t = t0;
    let mut rounds = 0;
    while r.w.round(t) {
        t += POLL;
        rounds += 1;
        assert!(rounds < 10_000, "polling never stopped");
    }
    assert!(t >= t0 + WATCH_FOR, "stopped early at {:?}", t - t0);
    assert!(t <= t0 + WATCH_FOR + POLL, "stopped late at {:?}", t - t0);
    let checks = r.fake.checks.load(Ordering::SeqCst);
    assert_eq!(checks as u64, WATCH_FOR.as_secs() / POLL.as_secs(), "one cheap check per round");
    let v = view(&r.w, t);
    assert_eq!((v.checking, v.line), (false, LINE_STOPPED));

    // Nothing more until someone asks.
    assert!(!r.w.round(t + Duration::from_secs(3600)));
    assert_eq!(r.fake.checks.load(Ordering::SeqCst), checks);

    // Check again: checks, no prompt, no window.
    r.w.flow.lock().unwrap().check_again(t);
    assert!(r.w.round(t));
    assert_eq!(r.fake.checks.load(Ordering::SeqCst), checks + 1);
    assert_eq!(r.fake.requests.load(Ordering::SeqCst), 1);
    assert!(r.fake.panes().is_empty());
    // The person allowed it while nobody checked: Check again sees it and
    // restarts K2 Screen.
    r.fake.set(tcc(true, true));
    assert!(!r.w.round(t + POLL));
    assert_eq!(r.fake.restarts.load(Ordering::SeqCst), 1);
}

#[test]
fn k2_screen_not_up_yet_asks_on_a_later_round() {
    let r = rig(Asked::default());
    let t0 = Instant::now();
    *r.fake.down.lock().unwrap() = true;
    r.w.flow.lock().unwrap().turned_on(t0);
    assert!(r.w.round(t0), "keeps trying");
    assert_eq!(r.fake.requests.load(Ordering::SeqCst), 0);
    assert_eq!(view(&r.w, t0).step, Step::Asking);
    *r.fake.down.lock().unwrap() = false;
    assert!(r.w.round(t0 + POLL));
    assert_eq!(r.fake.requests.load(Ordering::SeqCst), 1);
    assert_eq!(view(&r.w, t0 + POLL).step, Step::WaitingScreen);
}

#[test]
fn k2_screen_never_up_gives_up_after_the_limit() {
    let r = rig(Asked::default());
    let t0 = Instant::now();
    *r.fake.down.lock().unwrap() = true;
    r.w.flow.lock().unwrap().turned_on(t0);
    assert!(!r.w.round(t0 + WATCH_FOR + POLL), "past the limit: stop");
    assert!(!r.w.round(t0 + WATCH_FOR + POLL + POLL));
    assert_eq!(r.fake.requests.load(Ordering::SeqCst), 0);
}

#[test]
fn ask_at_the_mac_fires_again_and_opens_the_pane() {
    let r = rig(Asked::default());
    let t0 = Instant::now();
    r.w.flow.lock().unwrap().turned_on(t0);
    r.w.round(t0);
    assert!(r.fake.panes().is_empty());
    // Ask at the Mac (from another computer): macOS won't prompt twice,
    // so the pane opens.
    r.w.flow.lock().unwrap().ask(t0 + POLL);
    r.w.round(t0 + POLL);
    assert_eq!(r.fake.requests.load(Ordering::SeqCst), 2);
    assert_eq!(r.fake.panes(), vec![Pane::Screen]);
}

#[test]
fn the_watchers_ask_needs_the_switch() {
    let r = rig(Asked::default());
    assert!(!r.w.ask(Ask::Prompt), "off: nothing to ask");
    assert!(!r.w.ask(Ask::Check));
    assert!(!r.w.ask(Ask::Open(Pane::Screen)));
    assert!(r.fake.panes().is_empty());
    r.w.resume();
    assert!(r.w.ask(Ask::Open(Pane::Accessibility)));
    assert_eq!(r.fake.panes(), vec![Pane::Accessibility], "a step's button opens its pane");
    r.w.turned_off();
    assert!(!r.w.ask(Ask::Prompt));
    assert!(r.w.view(Some(NONE)).is_none());
}

#[test]
fn boot_with_the_switch_on_asks_nothing_and_shows_what_k2_screen_said() {
    let r = rig(Asked { screen: true, accessibility: true });
    r.w.resume();
    r.w.saw(tcc(false, true));
    assert!(!r.w.round(Instant::now()), "nothing asked or checked at boot");
    assert_eq!((r.fake.requests.load(Ordering::SeqCst), r.fake.checks.load(Ordering::SeqCst)), (0, 0));
    let v = r.w.view(None).expect("steps");
    assert_eq!((v.step, v.screen, v.accessibility, v.checking, v.line), (Step::WaitingScreen, "waiting", "allowed", false, LINE_STOPPED));
}

#[test]
fn no_steps_where_k2_screen_needs_no_permission() {
    let r = rig(Asked::default());
    r.w.resume();
    assert!(r.w.view(Some(Tcc::NOT_REQUIRED)).is_none(), "Linux: no macOS permission");
}

#[test]
fn the_view_serializes_for_status() {
    let r = rig(Asked { screen: true, accessibility: true });
    let t0 = Instant::now();
    r.w.flow.lock().unwrap().turned_on(t0);
    r.w.round(t0);
    let v = serde_json::to_value(view(&r.w, t0)).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"step": "waiting_screen", "screen": "waiting", "accessibility": "waiting", "opened": "screen", "checking": true, "line": LINE_OPENED})
    );
}

#[test]
fn pane_urls_and_names() {
    assert_eq!(Pane::Screen.url(), "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture");
    assert_eq!(Pane::Accessibility.url(), "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
    assert_eq!(Pane::parse("screen"), Some(Pane::Screen));
    assert_eq!(Pane::parse("accessibility"), Some(Pane::Accessibility));
    assert_eq!(Pane::parse("camera"), None);
    for line in [LINE_OPENED, LINE_PROMPTED, LINE_ASKING, LINE_STOPPED, LINE_WAITING, LINE_READY] {
        for jargon in ["TCC", "deskd", "tcc"] {
            assert!(!line.contains(jargon), "{line}");
        }
    }
}

#[test]
fn asked_round_trips_on_disk() {
    let _home = k2_core::test_env::TempHome::new();
    assert_eq!(read_asked(), Asked::default(), "never asked");
    write_asked(Asked { screen: true, accessibility: false }).expect("write");
    assert_eq!(read_asked(), Asked { screen: true, accessibility: false });
    assert!(asked_path().ends_with(".k2/desktop-permission.json"));
}
