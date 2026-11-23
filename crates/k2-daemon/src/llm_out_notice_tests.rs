//! Out of usage, daemon side: each harness's signal reaches the pipeline
//! once, the pipeline posts exactly one notice per event with the reset
//! time, and the words follow what happens next (switched, a login with
//! room, none). The world (agent, login, Thread write) is faked here; the
//! real Thread write is `overlay_routes` `out_of_usage_notice_tests`.

use super::*;

use k2_core::agent_hooks::envelope::{self, HookHeaders};
use serde_json::json;

fn mst() -> FixedOffset {
    FixedOffset::west_opt(7 * 3600).expect("offset")
}

fn now() -> i64 {
    Utc::now().timestamp()
}

struct Fake {
    login: Login,
    posts: Mutex<Vec<(String, String, SystemNotice)>>,
    fail: bool,
}

impl Fake {
    fn new(login: Login) -> Self {
        Self { login, posts: Mutex::new(Vec::new()), fail: false }
    }
    fn count(&self) -> usize {
        self.posts.lock().len()
    }
}

impl World for Fake {
    fn place(&self, _pane: &str, project_hint: Option<&str>) -> Option<Place> {
        Some(Place { addr: "sales".into(), workspace_id: project_hint.unwrap_or("ws-sales").into() })
    }
    fn login(&self, _place: &Place, _tool: Option<Tool>, _account: Option<&str>, _now: i64) -> Option<Login> {
        Some(self.login.clone())
    }
    fn post(&self, addr: &str, text: &str, notice: SystemNotice) -> Result<String, String> {
        if self.fail {
            return Err("thread is gone".into());
        }
        let mut p = self.posts.lock();
        p.push((addr.to_string(), text.to_string(), notice));
        Ok(format!("doc-{}", p.len()))
    }
    fn offset(&self, _at: i64) -> FixedOffset {
        mst()
    }
}

fn login(tool: Tool, next: Next) -> Login {
    Login { tool, id: Some("acc_1".into()), label: "Work Max".into(), api_key: false, reading_reset: None, next }
}

fn fresh() -> Mutex<ou::Episodes> {
    Mutex::new(ou::Episodes::default())
}

fn pane() -> String {
    format!("pane-{}", uuid::Uuid::new_v4())
}

/// The signal a hook handed over for `pane` (exactly one).
fn observed(pane: &str) -> Signal {
    let got: Vec<Signal> = OBSERVED.lock().iter().filter(|s| s.pane == pane).cloned().collect();
    assert_eq!(got.len(), 1, "exactly one signal for {pane}: {got:?}");
    got.into_iter().next().expect("one")
}

fn none_observed(pane: &str) {
    let got: Vec<Signal> = OBSERVED.lock().iter().filter(|s| s.pane == pane).cloned().collect();
    assert!(got.is_empty(), "no signal for {pane}: {got:?}");
}

fn claude_headers(pane: &str) -> HookHeaders {
    HookHeaders {
        pane: pane.to_string(),
        agent_pid: Some(4242),
        source: HookSource::Claude,
        hook_version: Some(2),
        cli_version: Some("2.1.292".to_string()),
        truncated: false,
        event_hint: None,
    }
}

fn stop_failure(pane: &str, error: &str) -> HookEnvelope {
    let body = json!({"hook_event_name": "StopFailure", "session_id": "conv-1", "error": error, "error_details": "Synthetic."});
    envelope::parse(&claude_headers(pane), body.to_string().as_bytes()).expect("parses")
}

/// Run one hook-observed signal through a fake world: the one post.
fn post_one(sig: &Signal, world: &Fake) -> Posted {
    let p = handle(world, &fresh(), sig, now()).expect("one notice");
    assert_eq!(world.count(), 1, "exactly one post");
    p
}

// ── Each harness's signal: one notice, with the reset time ──────────

#[test]
fn claude_stop_failure_gives_one_notice_with_the_reset() {
    let pane = pane();
    on_hook(&stop_failure(&pane, "rate_limit"));
    let sig = observed(&pane);
    assert_eq!((sig.tool, sig.hit.kind, sig.hit.source), (Some(Tool::Claude), OutKind::Usage, ou::SOURCE_STOP_FAILURE));
    // The hook carries no reset: the login's reading at its limit does.
    let reset = now() + 3 * 3600;
    let mut l = login(Tool::Claude, Next::Nothing);
    l.reading_reset = Some(reset);
    let world = Fake::new(l);
    let p = post_one(&sig, &world);
    let want = format!("sales is out of usage on Work Max until {}. Add or switch a login.", ou::when_words(reset, now(), mst()));
    assert_eq!(p.text, want);
    assert_eq!(p.resets_at, Some(reset));
    assert_eq!(world.posts.lock()[0].0, "sales", "on the agent's own Thread");
}

#[test]
fn claude_statusline_at_100_gives_one_notice_with_the_reset() {
    let pane = pane();
    let reset = now() + 2 * 3600;
    let body = json!({"k2_config_dir": "", "statusline": {"rate_limits": {
        "five_hour": {"used_percentage": 100.0, "resets_at": reset},
        "seven_day": {"used_percentage": 52.0, "resets_at": reset + 86_400}
    }}});
    on_statusline(&pane, body.to_string().as_bytes());
    let sig = observed(&pane);
    assert_eq!((sig.hit.source, sig.hit.resets_at, sig.account.clone()), (ou::SOURCE_STATUSLINE, Some(reset), None));
    let p = post_one(&sig, &Fake::new(login(Tool::Claude, Next::Nothing)));
    assert!(p.text.contains(&format!("until {}", ou::when_words(reset, now(), mst()))), "{}", p.text);
    // Below the limit: nothing reaches the pipeline.
    let other = self::pane();
    let low = json!({"statusline": {"rate_limits": {"five_hour": {"used_percentage": 1.0, "resets_at": reset}}}});
    on_statusline(&other, low.to_string().as_bytes());
    none_observed(&other);
}

#[test]
fn codex_rollout_at_100_gives_one_notice_with_the_reset() {
    let pane = pane();
    let reset = now() + 4 * 3600;
    let stamp = Utc::now().to_rfc3339();
    let lines = vec![
        json!({"timestamp": stamp, "type": "event_msg", "payload": {"type": "task_started"}}).to_string(),
        json!({"timestamp": stamp, "type": "event_msg", "payload": {"type": "token_count", "info": null,
            "rate_limits": {"primary": {"used_percent": 100.0, "window_minutes": 300, "resets_at": reset}}}})
        .to_string(),
    ];
    on_transcript_lines(&pane, "codex", Some("ws-codex"), &lines);
    let sig = observed(&pane);
    assert_eq!((sig.tool, sig.hit.source, sig.project_hint.as_deref()), (Some(Tool::Codex), ou::SOURCE_CODEX_ROLLOUT, Some("ws-codex")));
    let p = post_one(&sig, &Fake::new(login(Tool::Codex, Next::Nothing)));
    assert_eq!(p.resets_at, Some(reset));
    assert!(p.text.contains(&format!("until {}", ou::when_words(reset, now(), mst()))), "{}", p.text);
    assert_eq!((p.tool, p.workspace_id.as_str()), (Tool::Codex, "ws-codex"));
}

#[test]
fn claude_transcript_limit_record_gives_one_notice_with_the_reset() {
    let pane = pane();
    let reset = now() + 5400;
    let line = json!({"type": "assistant", "timestamp": Utc::now().to_rfc3339(), "isApiErrorMessage": true,
        "message": {"content": [{"type": "text", "text": format!("Claude AI usage limit reached|{reset}")}]}})
    .to_string();
    on_transcript_lines(&pane, "claude", None, &[line]);
    let sig = observed(&pane);
    let p = post_one(&sig, &Fake::new(login(Tool::Claude, Next::Nothing)));
    assert_eq!(p.resets_at, Some(reset));
}

#[test]
fn signals_that_are_not_running_out_never_reach_the_pipeline() {
    // Another error, a subagent's or a nested run's StopFailure.
    let p1 = pane();
    on_hook(&stop_failure(&p1, "server_error"));
    none_observed(&p1);
    let p2 = pane();
    let mut sub = stop_failure(&p2, "rate_limit");
    sub.agent_id = Some("sub-1".into());
    on_hook(&sub);
    none_observed(&p2);
    let p3 = pane();
    let mut nested = stop_failure(&p3, "rate_limit");
    nested.from_descendant = true;
    on_hook(&nested);
    none_observed(&p3);
    // An old rollout line read on catch-up is history.
    let p4 = pane();
    let old = json!({"timestamp": "2026-01-01T00:00:00Z", "type": "event_msg", "payload": {"type": "token_count",
        "rate_limits": {"primary": {"used_percent": 100.0}}}})
    .to_string();
    on_transcript_lines(&p4, "codex", None, &[old]);
    none_observed(&p4);
    // A harness with no reader.
    let p5 = pane();
    on_transcript_lines(&p5, "grok", None, &["{\"rate_limits\":1}".to_string()]);
    none_observed(&p5);
}

// ── One per event ───────────────────────────────────────────────────

#[test]
fn the_same_event_from_every_source_is_one_notice() {
    let world = Fake::new(login(Tool::Claude, Next::Nothing));
    let ep = fresh();
    let reset = now() + 3600;
    let mk = |hit: Hit| Signal { pane: "p".into(), project_hint: None, tool: Some(Tool::Claude), account: None, hit };
    let hook = mk(ou::claude_stop_failure(Some("rate_limit"), None).expect("hook"));
    let status = mk(ou::claude_statusline(&json!({"five_hour": {"used_percentage": 100, "resets_at": reset}})).expect("statusline"));
    assert!(handle(&world, &ep, &hook, now()).is_some(), "the first signal posts");
    assert!(handle(&world, &ep, &status, now() + 3).is_none(), "the statusLine for the same event doesn't");
    assert!(handle(&world, &ep, &hook, now() + 60).is_none(), "nor a second StopFailure (the agent retried)");
    assert_eq!(world.count(), 1);
    // The hook gate: a pane's second hook inside the gate isn't observed.
    let pane = pane();
    on_hook(&stop_failure(&pane, "rate_limit"));
    on_hook(&stop_failure(&pane, "rate_limit"));
    observed(&pane);
}

#[test]
fn a_failed_post_is_tried_again_on_the_next_signal() {
    let mut world = Fake::new(login(Tool::Claude, Next::Nothing));
    world.fail = true;
    let ep = fresh();
    let sig = Signal { pane: "p".into(), project_hint: None, tool: None, account: None, hit: ou::claude_stop_failure(Some("rate_limit"), None).expect("hit") };
    assert!(handle(&world, &ep, &sig, now()).is_none());
    world.fail = false;
    assert!(handle(&world, &ep, &sig, now() + 1).is_some(), "the claim was released");
    assert_eq!(world.count(), 1);
}

// ── What happens next ───────────────────────────────────────────────

fn hook_signal() -> Signal {
    Signal { pane: "p".into(), project_hint: None, tool: Some(Tool::Claude), account: None, hit: ou::claude_stop_failure(Some("rate_limit"), None).expect("hit") }
}

#[test]
fn auto_rotate_switched_wording() {
    let world = Fake::new(login(Tool::Claude, Next::Switched { label: "Home Pro".into() }));
    let p = handle(&world, &fresh(), &hook_signal(), now()).expect("notice");
    assert_eq!(p.text, "sales is out of usage on Work Max. Switched to Home Pro.");
    assert_eq!((p.code.as_str(), p.switched_to.as_deref()), (ou::CODE_SWITCHED, Some("Home Pro")));
    let (_, _, notice) = &world.posts.lock()[0];
    assert_eq!(notice.kind, "info");
    assert!(notice.actions.is_empty());
}

#[test]
fn no_other_login_wording_with_the_add_a_login_card() {
    let world = Fake::new(login(Tool::Claude, Next::Nothing));
    let p = handle(&world, &fresh(), &hook_signal(), now()).expect("notice");
    assert_eq!(p.text, "sales is out of usage on Work Max. Add or switch a login.");
    assert_eq!((p.code.as_str(), p.switched_to.as_deref()), (ou::CODE_NO_LOGIN, None));
    let (_, _, notice) = &world.posts.lock()[0];
    assert_eq!(notice.kind, "action");
    let ids: Vec<&str> = notice.actions.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, vec![ou::ACTION_ADD_LOGIN], "a person's one-tap card, not a Settings hunt");
    assert_eq!(notice.actions[0].body, Some(json!({"tool": "claude"})));
}

#[test]
fn a_login_with_room_offers_the_one_tap_switch() {
    let world = Fake::new(login(Tool::Claude, Next::Available { id: "acc_2".into(), label: "Home Pro".into() }));
    handle(&world, &fresh(), &hook_signal(), now()).expect("notice");
    let (_, text, notice) = &world.posts.lock()[0];
    assert!(text.ends_with("Home Pro has room: switch to it, or add a login."), "{text}");
    assert_eq!(notice.actions[0].route.as_deref(), Some(ou::SWITCH_ROUTE));
    assert_eq!(notice.actions[0].body, Some(json!({"id": "acc_2"})));
}

#[test]
fn an_api_keys_rate_limit_is_a_throttle_but_its_credit_is_out() {
    let mut l = login(Tool::Claude, Next::Nothing);
    l.api_key = true;
    let world = Fake::new(l);
    assert!(handle(&world, &fresh(), &hook_signal(), now()).is_none(), "429 on an API key: no notice");
    let credit = Signal { hit: ou::claude_stop_failure(Some("billing_error"), None).expect("billing"), ..hook_signal() };
    let p = handle(&world, &fresh(), &credit, now()).expect("credit notice");
    assert_eq!(p.text, "sales is out of credit on Work Max. Add or switch a login.");
}

#[test]
fn the_event_names_the_login_by_label_only() {
    let world = Fake::new(login(Tool::Claude, Next::Switched { label: "Home Pro".into() }));
    let p = handle(&world, &fresh(), &hook_signal(), now()).expect("notice");
    let mut rx = crate::session_events::subscribe();
    crate::session_events::emit_llm_out_of_usage(&p);
    let ev = loop {
        match rx.try_recv() {
            Ok(e @ crate::session_events::SessionEvent::LlmOutOfUsage { .. }) if e.kind_name() == "llm_out_of_usage" => {
                let v = serde_json::to_value(&e).expect("json");
                if v["noticeId"] == "doc-1" && v["address"] == "sales" {
                    break v;
                }
            }
            Ok(_) => continue,
            Err(e) => panic!("no llm_out_of_usage event: {e}"),
        }
    };
    assert_eq!(ev["kind"], "llm_out_of_usage");
    assert_eq!(ev["tool"], "claude");
    assert_eq!(ev["login"], "Work Max");
    assert_eq!(ev["switchedTo"], "Home Pro");
    assert_eq!(ev["workspaceId"], "ws-sales");
    let keys: Vec<&str> = ev.as_object().expect("object").keys().map(String::as_str).collect();
    assert_eq!(keys.len(), 8, "kind + seven fields, nothing else: {ev}");
    assert!(!ev.to_string().contains("acc_1"), "the login by its label only: {ev}");
}

// ── 0.46.18: working again ──────────────────────────────────────────

fn worked(started_s: i64, ended_s: i64) -> Worked {
    Worked { pane: "p".into(), project_hint: None, started_ms: started_s * 1000, ended_ms: ended_s * 1000 }
}

#[test]
fn repeated_detections_are_one_notice_and_working_again_is_one_line() {
    let world = Fake::new(login(Tool::Claude, Next::Nothing));
    let ep = fresh();
    let t = now();
    assert!(handle(&world, &ep, &hook_signal(), t).is_some());
    for i in 1..=10 {
        assert!(handle(&world, &ep, &hook_signal(), t + i * 20).is_none(), "detection {i}: same episode");
    }
    assert_eq!(world.count(), 1, "exactly one notice for eleven detections");
    assert_eq!(ep.lock().out_since("sales"), Some(t), "the agent is out from the notice");
    let doc = handle_worked(&world, &ep, &worked(t + 600, t + 700), None).expect("working again");
    assert_eq!(doc, "doc-2");
    {
        let posts = world.posts.lock();
        let (addr, text, notice) = &posts[1];
        assert_eq!((addr.as_str(), text.as_str()), ("sales", "sales is working again."));
        assert_eq!((notice.kind.as_str(), notice.code.as_str()), ("info", ou::CODE_RESUMED));
        assert!(notice.actions.is_empty());
    }
    assert!(handle_worked(&world, &ep, &worked(t + 800, t + 900), None).is_none(), "one line per episode");
    assert_eq!(world.count(), 2);
    // The next limit is a new episode: one more notice.
    assert!(handle(&world, &ep, &hook_signal(), t + 1000).is_some());
    assert!(handle(&world, &ep, &hook_signal(), t + 1010).is_none());
    assert_eq!(world.count(), 3);
}

#[test]
fn a_turn_that_hit_the_limit_is_not_working_again() {
    let world = Fake::new(login(Tool::Claude, Next::Nothing));
    let ep = fresh();
    let t = now();
    // Nobody is out: nothing.
    assert!(handle_worked(&world, &ep, &worked(t, t + 1), None).is_none());
    handle(&world, &ep, &hook_signal(), t).expect("notice");
    // The turn that ran out (started before the notice; a statusLine at
    // 100% mid-turn ends that turn well).
    assert!(handle_worked(&world, &ep, &worked(t - 30, t + 2), None).is_none());
    // A turn whose pane showed the limit again since it started.
    assert!(handle_worked(&world, &ep, &worked(t + 60, t + 70), Some((t + 65) * 1000)).is_none());
    assert!(handle_worked(&world, &ep, &worked(t + 60, t + 70), Some((t + 60) * 1000)).is_none(), "at the start counts");
    assert_eq!(world.count(), 1, "no working-again line yet");
    // A limit seen before this turn started doesn't count against it.
    assert!(handle_worked(&world, &ep, &worked(t + 100, t + 110), Some((t + 70) * 1000)).is_some());
    assert_eq!(world.count(), 2);
}

#[test]
fn a_failed_working_again_post_still_ends_the_episode() {
    let mut world = Fake::new(login(Tool::Claude, Next::Nothing));
    let ep = fresh();
    let t = now();
    handle(&world, &ep, &hook_signal(), t).expect("notice");
    world.fail = true;
    assert!(handle_worked(&world, &ep, &worked(t + 10, t + 20), None).is_none());
    world.fail = false;
    assert!(!ep.lock().any_out(), "the agent works: no line owed later");
    assert!(handle(&world, &ep, &hook_signal(), t + 30).is_some(), "its next limit is news");
}

#[test]
fn only_a_good_turn_end_with_a_start_is_worked() {
    use crate::activity_store::RowEvent;
    use k2_core::activity::{Outcome, Reason, TurnEnded};
    let ev = |te: Option<TurnEnded>, row: Option<serde_json::Value>| RowEvent {
        session_id: "pane-9".into(),
        agent_name: "ws-a".into(),
        workspace_path: None,
        row,
        turn_ended: te,
        holds_awake: false,
        evidence_at: None,
    };
    let row = json!({"sessionId": "pane-9", "projectId": "proj-1", "turnStartedAt": 1_000});
    let good = TurnEnded { outcome: Outcome::Success, reason: Reason::TurnDone, at: 5_000 };
    assert_eq!(
        worked_from(&ev(Some(good), Some(row.clone()))),
        Some(Worked { pane: "pane-9".into(), project_hint: Some("proj-1".into()), started_ms: 1_000, ended_ms: 5_000 })
    );
    let failed = TurnEnded { outcome: Outcome::Failure, reason: Reason::TurnFailed, at: 5_000 };
    assert_eq!(worked_from(&ev(Some(failed), Some(row.clone()))), None, "the red X: not working");
    let cut = TurnEnded { outcome: Outcome::Cancelled, reason: Reason::Interrupted, at: 5_000 };
    assert_eq!(worked_from(&ev(Some(cut), Some(row.clone()))), None, "an interrupt");
    assert_eq!(worked_from(&ev(None, Some(row))), None, "no turn end");
    assert_eq!(worked_from(&ev(Some(good), None)), None, "a removed row");
    assert_eq!(worked_from(&ev(Some(good), Some(json!({"projectId": "proj-1"})))), None, "no start");
}

#[test]
fn a_good_turn_end_waits_out_the_settle() {
    let mut s = Settle::default();
    assert!(s.is_empty());
    s.push(Worked { pane: "a".into(), project_hint: None, started_ms: 0, ended_ms: 1_000 });
    s.push(Worked { pane: "b".into(), project_hint: None, started_ms: 0, ended_ms: 4_000 });
    s.push(Worked { pane: "a".into(), project_hint: None, started_ms: 0, ended_ms: 2_000 });
    assert!(s.due(1_000 + RESUME_SETTLE_MS).is_empty(), "a's newer end replaced the older one");
    let due = s.due(2_000 + RESUME_SETTLE_MS);
    assert_eq!(due.iter().map(|w| w.pane.as_str()).collect::<Vec<_>>(), vec!["a"]);
    assert!(!s.is_empty());
    let due = s.due(4_000 + RESUME_SETTLE_MS);
    assert_eq!(due.iter().map(|w| w.pane.as_str()).collect::<Vec<_>>(), vec!["b"]);
    assert!(s.is_empty());
}

#[test]
fn every_limit_a_pane_shows_is_remembered_even_past_the_gate() {
    let pane = pane();
    assert_eq!(out_seen(&pane), None);
    let before = Utc::now().timestamp_millis();
    on_hook(&stop_failure(&pane, "rate_limit"));
    let first = out_seen(&pane).expect("seen");
    assert!(first >= before, "{first} >= {before}");
    on_hook(&stop_failure(&pane, "rate_limit"));
    assert!(out_seen(&pane).expect("seen again") >= first, "the gated second signal still counts");
    observed(&pane);
    // Not a limit: not seen.
    let other = self::pane();
    on_hook(&stop_failure(&other, "server_error"));
    assert_eq!(out_seen(&other), None);
}
