//! The single refresher (prd-usage-single-source-v1 §4.4, §11 and U8–U14,
//! U22, U24). The vendor is the fake in this file; no real vendor, CLI or
//! keychain is reached. Every test runs under one lock: the flight, gate
//! and back-off maps are process-wide.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use chrono::Duration as ChronoDuration;

use super::*;
use crate::usage_store::{pin, single_record_on};

fn serial() -> k2_core::test_env::SerialGuard {
    static LOCK: Mutex<()> = Mutex::new(());
    k2_core::test_env::serial(&LOCK)
}

const RESET_FAR: &str = "2099-01-01T00:00:00+00:00";

fn row(used: f64) -> HarnessUsage {
    HarnessUsage {
        harness: "claude".into(),
        plan: "Max".into(),
        windows: vec![su::window("five_hour", "Session", "session", used, RESET_FAR.to_string(), Some(18_000))],
        checked_at: Utc::now().to_rfc3339(),
        source: "probe".into(),
        ..Default::default()
    }
}

fn failed_row(err: &str) -> HarnessUsage {
    HarnessUsage { harness: "claude".into(), error: Some(err.into()), ..Default::default() }
}

/// A fake vendor. `live_row` / `slot_row` answer; `gate` (if any) holds the
/// call until released so a test can act while it runs.
#[derive(Default)]
struct FakeVendor {
    live_calls: AtomicUsize,
    slot_calls: AtomicUsize,
    live_row: Mutex<Option<HarnessUsage>>,
    slot_row: Mutex<Option<HarnessUsage>>,
    gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}

impl FakeVendor {
    fn hold(&self) {
        let g = self.gate.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some((entered, release)) = g {
            let _ = entered.send(());
            let _ = release.recv_timeout(Duration::from_secs(90));
        }
    }
}

impl Vendor for FakeVendor {
    fn live(&self, _tool: Tool) -> HarnessUsage {
        self.live_calls.fetch_add(1, Ordering::SeqCst);
        self.hold();
        self.live_row.lock().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_else(|| row(0.5))
    }

    fn slot(&self, _entry: &Entry) -> Option<HarnessUsage> {
        self.slot_calls.fetch_add(1, Ordering::SeqCst);
        self.hold();
        Some(self.slot_row.lock().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_else(|| row(0.3)))
    }
}

fn install(vendor: Arc<FakeVendor>) {
    *test_vendor_slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(vendor);
}

/// A gate on the fake's next call: `(entered, release)`.
fn hold_next(vendor: &FakeVendor) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (etx, erx) = mpsc::channel();
    let (rtx, rrx) = mpsc::channel();
    *vendor.gate.lock().unwrap_or_else(|p| p.into_inner()) = Some((etx, rrx));
    (erx, rtx)
}

struct Fixture {
    _serial: k2_core::test_env::SerialGuard,
    _db: k2_core::db::TestDbGuard,
    _pin: crate::usage_store::PinGuard,
    _airgap: k2_core::test_env::EnvVar,
    _home: crate::test_support::TempHome,
}

fn setup() -> Fixture {
    let serial = serial();
    let home = crate::test_support::TempHome::new();
    let fx = Fixture {
        _serial: serial,
        _db: k2_core::db::scoped_for_test(),
        _pin: pin(true),
        _airgap: k2_core::test_env::EnvVar::set("K2_AIRGAP", "0"),
        _home: home,
    };
    clear_for_tests();
    crate::subscription_usage::tests_clear_backoffs();
    fx
}

fn add_token(tool: Tool, label: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = wallet::create(&conn, tool, label, None).expect("create").id;
    wallet::update_meta(&conn, &id, &wallet::MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() }).expect("signed in");
    id
}

fn make_live(tool: Tool, id: &str, switched_at: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO llm_active (tool, account_id, switched_at, switched_by, live_account_id) VALUES (?1, ?2, ?3, NULL, NULL)",
        rusqlite::params![tool.as_str(), id, switched_at],
    )
    .expect("active");
}

fn stored_used(id: &str) -> Option<f64> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let e = wallet::get(&conn, id).expect("get").expect("row");
    let row: HarnessUsage = serde_json::from_str(e.usage_json.as_deref()?).expect("parses");
    Some(row.windows.first()?.used)
}

fn stored_row(id: &str) -> Option<HarnessUsage> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let e = wallet::get(&conn, id).expect("get").expect("row");
    Some(serde_json::from_str(e.usage_json.as_deref()?).expect("parses"))
}

fn wait_done(id: &str) {
    // A run that was not waited on: ask for its flight and wait for it.
    let flight = flights().lock().unwrap_or_else(|p| p.into_inner()).get(id).cloned();
    if let Some(f) = flight {
        assert_ne!(f.wait(Wait::For(Duration::from_secs(10))), Check::Pending, "the run finished");
    }
}

// ── Basic paths ─────────────────────────────────────────────────────

#[test]
fn a_live_token_is_read_through_the_live_store_and_recorded() {
    let _f = setup();
    let id = add_token(Tool::Claude, "live");
    make_live(Tool::Claude, &id, 1_000);
    let vendor = Arc::new(FakeVendor::default());
    install(vendor.clone());

    assert_eq!(request(&id, Why::Person, Wait::For(Duration::from_secs(10))), Outcome::Ran(Check::Ok));
    assert_eq!((vendor.live_calls.load(Ordering::SeqCst), vendor.slot_calls.load(Ordering::SeqCst)), (1, 0));
    assert_eq!(stored_used(&id), Some(0.5));
}

#[test]
fn an_idle_token_is_read_through_its_slot() {
    let _f = setup();
    let id = add_token(Tool::Claude, "idle");
    let vendor = Arc::new(FakeVendor::default());
    install(vendor.clone());
    assert_eq!(request(&id, Why::Person, Wait::For(Duration::from_secs(10))), Outcome::Ran(Check::Ok));
    assert_eq!((vendor.live_calls.load(Ordering::SeqCst), vendor.slot_calls.load(Ordering::SeqCst)), (0, 1));
    assert_eq!(stored_used(&id), Some(0.3));
}

#[test]
fn single_flight_a_second_caller_joins_and_only_one_vendor_call_runs() {
    let _f = setup();
    let id = add_token(Tool::Claude, "joined");
    let vendor = Arc::new(FakeVendor::default());
    let (entered, release) = hold_next(&vendor);
    install(vendor.clone());

    let first = {
        let (i2, scoped, pinned) = (id.clone(), k2_core::db::scoped_handle_for_test().expect("scoped"), crate::usage_store::current_pin());
        std::thread::spawn(move || {
            let _p = pinned.map(pin);
            k2_core::db::with_scoped_handle(scoped, || request(&i2, Why::Background, Wait::For(Duration::from_secs(10))))
        })
    };
    entered.recv_timeout(Duration::from_secs(5)).expect("the vendor call started");
    let joined = request(&id, Why::Person, Wait::For(Duration::from_millis(1)));
    assert_eq!(joined, Outcome::Joined(Check::Pending), "a run is going: join, don't start");
    drop(release);
    assert_eq!(first.join().expect("thread"), Outcome::Ran(Check::Ok));
    assert_eq!(vendor.slot_calls.load(Ordering::SeqCst), 1, "one vendor call");
}

#[test]
fn the_sixty_second_gate_spans_different_triggers() {
    let _f = setup();
    let id = add_token(Tool::Claude, "gated");
    let vendor = Arc::new(FakeVendor::default());
    install(vendor.clone());
    assert_eq!(request(&id, Why::Person, Wait::For(Duration::from_secs(10))), Outcome::Ran(Check::Ok));
    match request(&id, Why::Background, Wait::None) {
        Outcome::TooSoon(secs) => assert!((1..=60).contains(&secs), "{secs}"),
        other => panic!("expected TooSoon, got {other:?}"),
    }
    assert_eq!(vendor.slot_calls.load(Ordering::SeqCst), 1, "the second trigger made no vendor call");

    // Once the minute is up, it runs again.
    *admitted().lock().unwrap_or_else(|p| p.into_inner()).get_mut(&id).expect("stamped") -= Duration::from_secs(61);
    assert_eq!(request(&id, Why::Background, Wait::For(Duration::from_secs(10))), Outcome::Ran(Check::Ok));
    assert_eq!(vendor.slot_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn u13_the_gate_is_stamped_before_the_vendor_check() {
    let _f = setup();
    let id = add_token(Tool::Claude, "off");
    // No fake vendor, and probes off: the slot is taken, then "probes_off".
    let _deny = k2_core::test_env::EnvVar::set("K2_SUBSCRIPTION_PROBE", "deny");
    assert_eq!(request(&id, Why::Person, Wait::None), Outcome::Off("probes_off"));
    assert!(matches!(request(&id, Why::Person, Wait::None), Outcome::TooSoon(_)), "the second one says rate_limited");
}

#[test]
fn off_reasons_keep_todays_words() {
    let _f = setup();
    assert_eq!(request("no-such", Why::Person, Wait::None), Outcome::Off("unavailable"));
    let api = add_token(Tool::Claude, "api");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE llm_accounts SET kind = 'api_key' WHERE id = ?1", rusqlite::params![api]).expect("kind");
    }
    assert_eq!(request(&api, Why::Person, Wait::None), Outcome::Off("no_usage"));
    let out = add_token(Tool::Claude, "out");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE llm_accounts SET state = 'needs_login' WHERE id = ?1", rusqlite::params![out]).expect("state");
    }
    assert_eq!(request(&out, Why::Person, Wait::None), Outcome::Off("not_signed_in"));
}

#[test]
fn u9_a_live_token_is_checked_whatever_its_state() {
    let _f = setup();
    let id = add_token(Tool::Claude, "live-needs-login");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE llm_accounts SET state = 'needs_login' WHERE id = ?1", rusqlite::params![id]).expect("state");
    }
    make_live(Tool::Claude, &id, 1_000);
    let vendor = Arc::new(FakeVendor::default());
    install(vendor.clone());
    assert_eq!(request(&id, Why::Background, Wait::For(Duration::from_secs(10))), Outcome::Ran(Check::Ok));
    assert_eq!(vendor.live_calls.load(Ordering::SeqCst), 1);
}

// ── The freeze ──────────────────────────────────────────────────────

#[test]
fn a_request_with_the_database_held_answers_busy_and_does_not_wait() {
    let _f = setup();
    let id = add_token(Tool::Claude, "busy");
    let vendor = Arc::new(FakeVendor::default());
    install(vendor.clone());
    let db = k2_core::db::shared();
    let _conn = db.lock();
    assert_eq!(request(&id, Why::Person, Wait::UntilDone), Outcome::Off("busy"));
    assert_eq!(vendor.slot_calls.load(Ordering::SeqCst), 0, "no vendor call under the database lock");
}

/// While `vendor` holds its call: a database read finishes promptly, and a
/// waiting request gives up within its own timeout with no lock held.
fn assert_nothing_waits_on_a_hanging_vendor(id: &str, vendor: Arc<FakeVendor>) {
    let (entered, release) = hold_next(&vendor);
    install(vendor.clone());
    let id2 = id.to_string();
    let scoped = k2_core::db::scoped_handle_for_test().expect("a scoped db");
    let pinned = crate::usage_store::current_pin();
    let answering = std::thread::spawn(move || {
        let _p = pinned.map(pin);
        k2_core::db::with_scoped_handle(scoped, || {
            let t = Instant::now();
            let out = request(&id2, Why::Person, Wait::For(Duration::from_millis(300)));
            (out, t.elapsed())
        })
    });
    entered.recv_timeout(Duration::from_secs(5)).expect("the vendor call started");
    let t = Instant::now();
    let db = k2_core::db::shared();
    let n: i64 = db.lock().query_row("SELECT COUNT(*) FROM llm_accounts", [], |r| r.get(0)).expect("query");
    assert!(n >= 1);
    assert!(t.elapsed() < Duration::from_secs(1), "a database read waited {:?}", t.elapsed());
    let (out, took) = answering.join().expect("thread");
    assert_eq!(out, Outcome::Ran(Check::Pending), "a person's request gives up its own wait; the run is still going");
    assert!(took < Duration::from_secs(5), "waited {took:?}");
    drop(release);
    wait_done(id);
}

#[test]
fn a_hanging_live_vendor_blocks_neither_the_database_nor_a_request() {
    let _f = setup();
    let id = add_token(Tool::Claude, "hang-live");
    make_live(Tool::Claude, &id, 1_000);
    assert_nothing_waits_on_a_hanging_vendor(&id, Arc::new(FakeVendor::default()));
}

#[test]
fn a_hanging_slot_vendor_blocks_neither_the_database_nor_a_request() {
    let _f = setup();
    let id = add_token(Tool::Claude, "hang-slot");
    assert_nothing_waits_on_a_hanging_vendor(&id, Arc::new(FakeVendor::default()));
}

// ── Competing writers around a switch (§11) ─────────────────────────

fn events(rx: &mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>, id: &str) -> usize {
    let mut n = 0;
    while let Ok(ev) = rx.try_recv() {
        if let crate::session_events::SessionEvent::SubscriptionUsageChanged { account_id, .. } = ev {
            if account_id.as_deref() == Some(id) {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn a_switch_during_a_live_check_drops_the_reading_for_both_tokens_with_no_event() {
    let _f = setup();
    let a = add_token(Tool::Claude, "a");
    let b = add_token(Tool::Claude, "b");
    make_live(Tool::Claude, &a, 1_000);
    let vendor = Arc::new(FakeVendor::default());
    let (entered, release) = hold_next(&vendor);
    install(vendor.clone());
    let mut rx = crate::session_events::subscribe();

    let waiting = {
        let (a2, scoped, pinned) = (a.clone(), k2_core::db::scoped_handle_for_test().expect("scoped"), crate::usage_store::current_pin());
        std::thread::spawn(move || {
            let _p = pinned.map(pin);
            k2_core::db::with_scoped_handle(scoped, || request(&a2, Why::Person, Wait::For(Duration::from_secs(10))))
        })
    };
    entered.recv_timeout(Duration::from_secs(5)).expect("the check started");
    make_live(Tool::Claude, &b, 2_000);
    drop(release);
    assert_eq!(waiting.join().expect("thread"), Outcome::Ran(Check::Ok));

    assert_eq!(stored_used(&a), None, "not the old token");
    assert_eq!(stored_used(&b), None, "not the new token");
    assert_eq!((events(&mut rx, &a), events(&mut rx, &b)), (0, 0), "no event for a dropped reading");
}

#[test]
fn a_switch_to_a_token_while_its_slot_check_runs_keeps_the_reading_on_that_token() {
    let _f = setup();
    let a = add_token(Tool::Claude, "a");
    let b = add_token(Tool::Claude, "b");
    make_live(Tool::Claude, &a, 1_000);
    let vendor = Arc::new(FakeVendor::default());
    let (entered, release) = hold_next(&vendor);
    install(vendor.clone());
    let waiting = {
        let (b2, scoped, pinned) = (b.clone(), k2_core::db::scoped_handle_for_test().expect("scoped"), crate::usage_store::current_pin());
        std::thread::spawn(move || {
            let _p = pinned.map(pin);
            k2_core::db::with_scoped_handle(scoped, || request(&b2, Why::Person, Wait::For(Duration::from_secs(10))))
        })
    };
    entered.recv_timeout(Duration::from_secs(5)).expect("the check started");
    make_live(Tool::Claude, &b, 2_000);
    drop(release);
    assert_eq!(waiting.join().expect("thread"), Outcome::Ran(Check::Ok));
    assert_eq!(stored_used(&b), Some(0.3), "the slot's credential is that token's; the reading stays");
    assert_eq!(stored_used(&a), None);
}

#[test]
fn a_live_reading_while_the_live_sign_in_is_another_account_is_dropped() {
    let _f = setup();
    let id = add_token(Tool::Claude, "other");
    make_live(Tool::Claude, &id, 1_000);
    let vendor = Arc::new(FakeVendor::default());
    install(vendor.clone());
    wallet::wallet::set_live_verdict_for_test(Tool::Claude, true);
    let out = request(&id, Why::Person, Wait::For(Duration::from_secs(10)));
    wallet::wallet::set_live_verdict_for_test(Tool::Claude, false);
    assert_eq!(out, Outcome::Ran(Check::Ok));
    assert_eq!(stored_used(&id), None);
}

// ── Back-off (U8, U11) ──────────────────────────────────────────────

fn set_slot_row(vendor: &FakeVendor, row: HarnessUsage) {
    *vendor.slot_row.lock().unwrap_or_else(|p| p.into_inner()) = Some(row);
}

fn set_live_row(vendor: &FakeVendor, row: HarnessUsage) {
    *vendor.live_row.lock().unwrap_or_else(|p| p.into_inner()) = Some(row);
}

fn next_minute(id: &str) {
    *admitted().lock().unwrap_or_else(|p| p.into_inner()).get_mut(id).expect("stamped") -= Duration::from_secs(61);
}

#[test]
fn back_off_follows_the_token_across_a_switch() {
    let _f = setup();
    let a = add_token(Tool::Claude, "a");
    let b = add_token(Tool::Claude, "b");
    make_live(Tool::Claude, &a, 1_000);
    let vendor = Arc::new(FakeVendor::default());
    set_live_row(&vendor, failed_row("usage endpoint status 503"));
    install(vendor.clone());

    assert_eq!(request(&a, Why::Background, Wait::For(Duration::from_secs(10))), Outcome::Ran(Check::Failed));
    assert!(su::backing_off(&backoff_key(&a), Utc::now()).is_some(), "the failure backs off the token");

    // A switch moves the token off the live store; its back-off stays on it,
    // and the new live token has none.
    make_live(Tool::Claude, &b, 2_000);
    next_minute(&a);
    assert!(matches!(request(&a, Why::Background, Wait::None), Outcome::BackingOff(_)), "background honours it after a switch");
    assert!(su::backing_off(&backoff_key(&b), Utc::now()).is_none(), "the new live token isn't backed off");
    let tick = request(&b, Why::Background, Wait::For(Duration::from_secs(10)));
    assert_eq!(tick, Outcome::Ran(Check::Failed), "the failure on b is b's own");
}

#[test]
fn a_person_ignores_failure_back_off_but_waits_out_a_429() {
    let _f = setup();
    let id = add_token(Tool::Claude, "backoff");
    let vendor = Arc::new(FakeVendor::default());
    set_slot_row(&vendor, failed_row("usage endpoint status 500"));
    install(vendor.clone());
    assert_eq!(request(&id, Why::Person, Wait::For(Duration::from_secs(10))), Outcome::Ran(Check::Failed));

    next_minute(&id);
    set_slot_row(&vendor, row(0.3));
    assert!(matches!(request(&id, Why::Background, Wait::None), Outcome::BackingOff(_)), "background honours the failure back-off");
    assert_eq!(request(&id, Why::Person, Wait::For(Duration::from_secs(10))), Outcome::Ran(Check::Ok), "a person's refresh ignores it");
    assert!(su::backing_off(&backoff_key(&id), Utc::now()).is_none(), "success clears it");

    next_minute(&id);
    set_slot_row(&vendor, failed_row("usage endpoint status 429 retry-after 3600"));
    assert_eq!(request(&id, Why::Background, Wait::For(Duration::from_secs(10))), Outcome::Ran(Check::Failed));
    next_minute(&id);
    assert!(matches!(request(&id, Why::Person, Wait::None), Outcome::BackingOff(_)), "a 429 stops even a person");
}

#[test]
fn u8_a_slot_failure_is_dropped_when_the_token_went_live_during_the_check() {
    let _f = setup();
    let a = add_token(Tool::Claude, "a");
    let b = add_token(Tool::Claude, "b");
    make_live(Tool::Claude, &a, 1_000);
    let vendor = Arc::new(FakeVendor::default());
    set_slot_row(&vendor, failed_row("usage endpoint status 401"));
    let (entered, release) = hold_next(&vendor);
    install(vendor.clone());
    let waiting = {
        let (b2, scoped, pinned) = (b.clone(), k2_core::db::scoped_handle_for_test().expect("scoped"), crate::usage_store::current_pin());
        std::thread::spawn(move || {
            let _p = pinned.map(pin);
            k2_core::db::with_scoped_handle(scoped, || request(&b2, Why::Person, Wait::For(Duration::from_secs(10))))
        })
    };
    entered.recv_timeout(Duration::from_secs(5)).expect("the check started");
    make_live(Tool::Claude, &b, 2_000);
    drop(release);
    assert_eq!(waiting.join().expect("thread"), Outcome::Ran(Check::Failed));
    assert!(su::backing_off(&backoff_key(&b), Utc::now()).is_none(), "no back-off on the token now live");
    assert_eq!(stored_row(&b), None, "no error on its row");
}

#[test]
fn u8_a_failure_older_than_an_accepted_reading_leaves_no_back_off() {
    let _f = setup();
    let id = add_token(Tool::Claude, "race");
    make_live(Tool::Claude, &id, 1_000);
    let vendor = Arc::new(FakeVendor::default());
    set_live_row(&vendor, failed_row("usage endpoint status 500"));
    let (entered, release) = hold_next(&vendor);
    install(vendor.clone());
    let waiting = {
        let (i2, scoped, pinned) = (id.clone(), k2_core::db::scoped_handle_for_test().expect("scoped"), crate::usage_store::current_pin());
        std::thread::spawn(move || {
            let _p = pinned.map(pin);
            k2_core::db::with_scoped_handle(scoped, || request(&i2, Why::Person, Wait::For(Duration::from_secs(10))))
        })
    };
    entered.recv_timeout(Duration::from_secs(5)).expect("the check started");
    std::thread::sleep(Duration::from_millis(5));
    let feed = crate::usage_store::Reading {
        kind: crate::usage_store::Kind::Feed,
        measured: Utc::now(),
        source: "live_statusline".into(),
        plan: String::new(),
        body: crate::usage_store::Body::Windows(vec![su::window("five_hour", "Session", "session", 0.6, RESET_FAR.to_string(), Some(18_000))]),
    };
    assert!(matches!(crate::usage_store::accept(&id, feed), crate::usage_store::Outcome::Accepted { .. }));
    drop(release);
    assert_eq!(waiting.join().expect("thread"), Outcome::Ran(Check::Failed));
    assert!(su::backing_off(&backoff_key(&id), Utc::now()).is_none(), "the failure and its back-off are dropped");
    assert_eq!(stored_used(&id), Some(0.6));
}

// ── The tick (U9) ───────────────────────────────────────────────────

fn set_row(id: &str, row: &HarnessUsage) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    wallet::update_meta(&conn, id, &wallet::MetaUpdate { usage_json: Some(serde_json::to_string(row).expect("json")), ..Default::default() }).expect("seed");
}

#[test]
fn u9_the_tick_skips_tokens_the_vendor_read_lately_not_ones_a_feed_read() {
    let _f = setup();
    let now = Utc::now();
    let vendor_fresh = add_token(Tool::Claude, "vendor-fresh");
    set_row(&vendor_fresh, &HarnessUsage { vendor_checked_at: Some((now - ChronoDuration::minutes(5)).to_rfc3339()), ..row(0.1) });
    let feed_fresh = add_token(Tool::Claude, "feed-fresh");
    set_row(&feed_fresh, &HarnessUsage { source: "live_statusline".into(), checked_at: now.to_rfc3339(), ..row(0.1) });
    let old_probe = add_token(Tool::Claude, "old-probe");
    set_row(&old_probe, &HarnessUsage { source: "probe".into(), checked_at: (now - ChronoDuration::minutes(30)).to_rfc3339(), vendor_checked_at: None, ..row(0.1) });
    let recent_old_row = add_token(Tool::Claude, "recent-old-row");
    set_row(&recent_old_row, &HarnessUsage { source: "probe".into(), checked_at: now.to_rfc3339(), vendor_checked_at: None, ..row(0.1) });
    let never = add_token(Tool::Claude, "never");

    let due = tick_tokens(now);
    assert!(!due.contains(&vendor_fresh), "the vendor read it 5 minutes ago");
    assert!(due.contains(&feed_fresh), "a feed never counts as a vendor check");
    assert!(due.contains(&old_probe));
    assert!(!due.contains(&recent_old_row), "an old row's probe time counts");
    assert!(due.contains(&never));
}

#[test]
fn u9_the_tick_includes_a_live_owner_in_any_state_and_excludes_api_tokens_and_signed_out_idle_ones() {
    let _f = setup();
    let live = add_token(Tool::Claude, "live");
    let idle_out = add_token(Tool::Claude, "idle-out");
    let api = add_token(Tool::Claude, "api");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE llm_accounts SET state = 'needs_login' WHERE id IN (?1, ?2)", rusqlite::params![live, idle_out]).expect("state");
        conn.execute("UPDATE llm_accounts SET kind = 'api_key' WHERE id = ?1", rusqlite::params![api]).expect("kind");
    }
    make_live(Tool::Claude, &live, 1_000);
    let due = tick_tokens(Utc::now());
    assert!(due.contains(&live), "a live token marked needs_login is still read");
    assert!(!due.contains(&idle_out), "an idle token that isn't signed in is not");
    assert!(!due.contains(&api), "never an API token");
}

// ── Adoption (U15) ──────────────────────────────────────────────────

#[test]
fn a_login_made_after_boot_is_adopted_then_its_reading_lands() {
    let f = setup();
    let home = f._home.path().to_path_buf();
    let dir = home.join(".claude");
    std::fs::create_dir_all(&dir).expect(".claude");
    let cred = r#"{"claudeAiOauth":{"accessToken":"sk-ant-o-access-test","refreshToken":"sk-ant-r-refresh-test","expiresAt":4102444800000,"rateLimitTier":"max_20x","subscriptionType":"max"}}"#;
    std::fs::write(dir.join(".credentials.json"), cred).expect("live cred");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(wallet::active_id(&conn, Tool::Claude).expect("active"), None);
    }

    crate::llm_accounts_runtime::adopt_live_logins();

    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        wallet::active_id(&conn, Tool::Claude).expect("active").expect("adopted as the active token")
    };
    let vendor = Arc::new(FakeVendor::default());
    install(vendor.clone());
    assert_eq!(request(&id, Why::Background, Wait::For(Duration::from_secs(10))), Outcome::Ran(Check::Ok));
    assert_eq!(vendor.live_calls.load(Ordering::SeqCst), 1, "the adopted token is the live one");
    assert_eq!(stored_used(&id), Some(0.5));

    // A second pass with an active token does nothing.
    crate::llm_accounts_runtime::adopt_live_logins();
}

// ── U12: the same words on /for ─────────────────────────────────────

// ── The switch ──────────────────────────────────────────────────────

#[test]
fn the_tests_run_with_the_switch_on() {
    let _f = setup();
    assert!(single_record_on());
}

#[test]
fn keep_warm_probes_no_usage_with_the_switch_on() {
    let _f = setup();
    let id = add_token(Tool::Claude, "kw");
    let entry = entry_of(&id).expect("entry");
    assert!(crate::llm_accounts_runtime::keep_warm_usage_entries_for_tests(std::slice::from_ref(&entry)).is_empty());
    let _off = pin(false);
    assert_eq!(crate::llm_accounts_runtime::keep_warm_usage_entries_for_tests(std::slice::from_ref(&entry)).len(), 1, "off = today's probe of idle logins");
}

// ── Source guards (§4.4, U22, U24) ──────────────────────────────────

fn code_of(path: &str) -> String {
    let text = std::fs::read_to_string(format!("{}/src/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{path}: {e}"));
    // Test modules and comments don't count.
    let body = text.split("#[cfg(test)]\n#[path").next().expect("split");
    body.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

#[test]
fn neither_new_module_takes_a_tool_lock_or_a_tool_db() {
    for file in ["usage_refresh.rs", "usage_store.rs"] {
        let code = code_of(file);
        assert!(!code.contains("lock_tool"), "{file} takes a tool lock directly");
        assert!(!code.contains("with_tool_db"), "{file} takes a tool lock directly");
    }
}

#[test]
fn the_vendor_calls_assert_no_lock_is_held() {
    let code = code_of("usage_refresh.rs");
    assert!(
        code.matches("!store::db_held_here() && !store::tool_lock_held_here(tool)").count() >= 2,
        "both vendor calls assert no lock (U24)"
    );
}

#[test]
fn only_usage_store_writes_usage_json_outside_the_old_path() {
    // `usage_json: Some` is the writer's mark. The old path keeps two sites
    // until S7 (U3); nothing else may gain one.
    let allowed = [("subscription_usage.rs", 1), ("llm_accounts_runtime.rs", 1), ("usage_store.rs", 1)];
    let src = format!("{}/src", env!("CARGO_MANIFEST_DIR"));
    let mut found: Vec<(String, usize)> = Vec::new();
    for entry in walk(&src) {
        let text = std::fs::read_to_string(&entry).expect("read source");
        let name = entry.strip_prefix(&src).expect("under src").trim_start_matches('/').to_string();
        if name.ends_with("_tests.rs") || name.contains("/tests/") {
            continue;
        }
        let code = non_test_code(&text);
        let n = code.matches("usage_json: Some").count();
        if n > 0 {
            found.push((name, n));
        }
    }
    found.sort();
    let want: Vec<(String, usize)> = allowed.iter().map(|(f, n)| ((*f).to_string(), *n)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    assert_eq!(found, want, "a new writer of usage_json: route it through usage_store::accept");
}

/// The file up to its test module (a `#[cfg(test)]` on a single item
/// earlier in the file doesn't end the production code).
fn non_test_code(text: &str) -> &str {
    let end = ["#[cfg(test)]
mod ", "#[cfg(test)]
pub(crate) mod ", "#[cfg(test)]
#[path"]
        .iter()
        .filter_map(|m| text.find(m))
        .min()
        .unwrap_or(text.len());
    &text[..end]
}

fn walk(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read src") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            out.extend(walk(&path.to_string_lossy()));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path.to_string_lossy().into_owned());
        }
    }
    out
}

// ── U3: with the switch off, the new paths don't run ────────────────

#[test]
fn switch_off_adoption_does_nothing() {
    let f = setup();
    let _off = pin(false);
    let dir = f._home.path().join(".claude");
    std::fs::create_dir_all(&dir).expect(".claude");
    std::fs::write(dir.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"sk-ant-o-a","refreshToken":"sk-ant-r-r","expiresAt":4102444800000}}"#).expect("live cred");
    crate::llm_accounts_runtime::spawn_adopt_live_logins();
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(wallet::active_id(&conn, Tool::Claude).expect("active"), None, "off: the tick adopts nothing");
}

#[test]
fn switch_off_keep_warm_still_probes_idle_tokens() {
    let _f = setup();
    let _off = pin(false);
    let id = add_token(Tool::Claude, "kw-off");
    let entry = entry_of(&id).expect("entry");
    assert_eq!(crate::llm_accounts_runtime::keep_warm_usage_entries_for_tests(std::slice::from_ref(&entry)).len(), 1);
}
