//! Speed pass P5: the refresh never waits on a vendor, and the vendor
//! checks run at once, each with its own timeout.

use super::refresh_async::wants_wait;
use super::tests::{future_expiry_secs, future_reset, harness, install, usage_body, with_isolated, PanicIo};
use super::*;

/// A fake vendor: each tool answers after its delay; Claude's GET can
/// also be held until the test drops the sender.
struct TimedIo {
    delays: [Duration; 3],
    hold_claude: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl TimedIo {
    fn new(delays: [Duration; 3], hold: Option<std::sync::mpsc::Receiver<()>>) -> Arc<TimedIo> {
        Arc::new(TimedIo { delays, hold_claude: Mutex::new(hold) })
    }
}

fn open_window(used: f64) -> UsageWindow {
    window("five_hour", "Session", "session", used, future_reset(Utc::now()), Some(18_000))
}

impl UsageIo for TimedIo {
    fn claude_login(&self) -> ClaudeLogin {
        ClaudeLogin::Token { access_token: "T".into(), expires_at: Some(future_expiry_secs()), plan: "Max 20x".into() }
    }
    fn claude_get(&self, _t: &str) -> Result<String, String> {
        let held = self.hold_claude.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(rx) = held {
            // Released (or the sender dropped) by the test.
            let _ = rx.recv_timeout(Duration::from_secs(60));
        }
        std::thread::sleep(self.delays[0]);
        Ok(usage_body())
    }
    fn codex(&self) -> CodexOutcome {
        std::thread::sleep(self.delays[1]);
        CodexOutcome::Ready { plan: "Plus".into(), windows: vec![open_window(0.2)] }
    }
    fn grok(&self) -> GrokOutcome {
        std::thread::sleep(self.delays[2]);
        GrokOutcome::Ready { plan: "SuperGrok".into(), windows: vec![open_window(0.3)] }
    }
}

fn body_json(r: &CliResponse) -> Value {
    assert_eq!(r.status, "200 OK", "{}", r.body);
    serde_json::from_str(&r.body).expect("json body")
}

#[test]
fn p5_the_refresh_answers_the_saved_numbers_while_a_vendor_hangs() {
    with_isolated(|| {
        let stale = (Utc::now() - chrono::Duration::seconds(16)).to_rfc3339();
        let saved = |name: &str| harness(name, vec![open_window(0.42)], "", &stale);
        write_cache(&SubscriptionFile { harnesses: vec![saved(HARNESS_CLAUDE), saved(HARNESS_CODEX), saved(HARNESS_GROK)] })
            .expect("seed");
        let (release, held) = std::sync::mpsc::channel::<()>();
        install(TimedIo::new([Duration::ZERO; 3], Some(held)));

        let started = Instant::now();
        let first = handle_refresh_route("");
        let took = started.elapsed();
        let v = body_json(&first);
        assert!(took < Duration::from_millis(200), "the menu's refresh answered in {took:?}");
        assert_eq!(v["refreshing"], json!(true), "{v}");
        assert_eq!(v["harnesses"][0]["harness"], json!(HARNESS_CLAUDE), "{v}");
        assert_eq!(v["harnesses"][0]["windows"][0]["usedPercent"], json!(42.0), "the saved numbers: {v}");
        assert_eq!(v["harnesses"][0]["checkedAt"], json!(stale), "marked with their age: {v}");

        // Opening again while the check hangs: at once, and no second check.
        let started = Instant::now();
        let again = body_json(&handle_refresh_route(""));
        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(again["refreshing"], json!(true));
        assert_eq!(body_json(&handle_get())["refreshing"], json!(true), "the GET says a check is running");

        drop(release);
        assert!(wait_refresh_idle(Duration::from_secs(30)), "the background check ends");
        assert_eq!(probe_hits().load(Ordering::SeqCst), 1, "one check for both opens");

        // The saved values are the new ones, and the mark is gone.
        let after = body_json(&handle_get());
        assert_eq!(after["refreshing"], json!(false), "{after}");
        let written = read_cache_file().expect("cache written");
        let row = |name: &str| written.harnesses.iter().find(|h| h.harness == name).cloned().expect("row");
        let claude = row(HARNESS_CLAUDE);
        assert_eq!(claude.error, None, "{claude:?}");
        assert_ne!(claude.checked_at, stale, "claude was read again");
        assert_eq!(row(HARNESS_CODEX).windows[0].used, 0.2);
        assert_eq!(row(HARNESS_GROK).windows[0].used, 0.3);
    });
}

#[test]
fn p5_wait_1_keeps_the_answer_that_waits_for_the_check() {
    with_isolated(|| {
        install(TimedIo::new([Duration::from_millis(300), Duration::ZERO, Duration::ZERO], None));
        let started = Instant::now();
        let v = body_json(&handle_refresh_route("wait=1"));
        assert!(started.elapsed() >= Duration::from_millis(300), "it waited for the check");
        assert_eq!(v["refreshing"], json!(false), "{v}");
        assert_eq!(v["harnesses"][1]["windows"][0]["usedPercent"], json!(20.0), "the new numbers: {v}");
        assert_eq!(probe_hits().load(Ordering::SeqCst), 1);
        assert!(wants_wait("?wait=1") && wants_wait("a=b&wait=true"));
        assert!(!wants_wait("") && !wants_wait("wait=0") && !wants_wait("nowait=1"));
    });
}

#[test]
fn p5_a_fresh_cache_starts_no_background_check() {
    with_isolated(|| {
        install(Arc::new(PanicIo));
        let now = Utc::now().to_rfc3339();
        let saved = |name: &str| harness(name, Vec::new(), STATUS_NOT_SIGNED_IN, &now);
        write_cache(&SubscriptionFile { harnesses: vec![saved(HARNESS_CLAUDE), saved(HARNESS_CODEX), saved(HARNESS_GROK)] })
            .expect("seed");
        let v = body_json(&handle_refresh_route(""));
        assert_eq!(v["refreshing"], json!(false), "{v}");
        assert!(wait_refresh_idle(Duration::from_secs(5)));
        assert_eq!(probe_hits().load(Ordering::SeqCst), 0);
    });
}

#[test]
fn p5_the_three_checks_run_at_once() {
    with_isolated(|| {
        let each = Duration::from_secs(2);
        let started = Instant::now();
        let doc = probe_and_write(TimedIo::new([each; 3], None)).expect("probe");
        let took = started.elapsed();
        assert!(took < Duration::from_secs(3), "three 2 s checks took {took:?} (about the slowest, not the sum)");
        assert_eq!(doc.harnesses.len(), 3);
        for h in &doc.harnesses {
            assert_eq!(h.error, None, "{h:?}");
        }
    });
}

#[test]
fn p5_one_check_that_hangs_does_not_hide_the_others() {
    with_isolated(|| {
        *test_vendor_timeout_slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(Duration::from_millis(300));
        let old = (Utc::now() - chrono::Duration::minutes(20)).to_rfc3339();
        write_cache(&SubscriptionFile { harnesses: vec![harness(HARNESS_CLAUDE, vec![open_window(0.55)], "", &old)] })
            .expect("seed");
        let (release, held) = std::sync::mpsc::channel::<()>();
        let started = Instant::now();
        let doc = probe_and_write(TimedIo::new([Duration::ZERO; 3], Some(held))).expect("probe");
        let took = started.elapsed();
        assert!(took < Duration::from_secs(2), "the hung check was cut at its own timeout: {took:?}");
        let row = |name: &str| doc.harnesses.iter().find(|h| h.harness == name).cloned().expect("row");
        let claude = row(HARNESS_CLAUDE);
        assert_eq!(claude.error.as_deref(), Some("claude did not answer in time"), "{claude:?}");
        assert_eq!(claude.windows.len(), 1, "the last reading is kept: {claude:?}");
        assert_eq!(claude.windows[0].used, 0.55);
        assert_eq!(claude.checked_at, old, "and keeps its own time");
        let codex = row(HARNESS_CODEX);
        assert_eq!((codex.error.clone(), codex.windows[0].used), (None, 0.2), "{codex:?}");
        let grok = row(HARNESS_GROK);
        assert_eq!((grok.error.clone(), grok.windows[0].used), (None, 0.3), "{grok:?}");
        assert_eq!(read_cache_file().expect("written"), doc, "the cache holds the same rows");
        drop(release);
    });
}
